import { spawn, type ChildProcessWithoutNullStreams } from 'node:child_process';
import { MAX_PROVIDER_INPUT_UTF8_BYTES } from '../../shared/schemas/providers';
import { ProviderError } from './errors';
import { PiRpcTransport, type PiRpcOutboundCommand } from './pi-rpc-transport';
import {
  PiRpcTimingStage,
  type PiRpcExpectedState,
  type PiRpcPrewarmOptions,
  type PiRpcPromptResult,
} from './pi-rpc-types';
import { spawnPiRpc, validatePrewarmOptions } from './pi-rpc-config';
import { PiRpcRetirement } from './pi-rpc-retirement';
import { PiRpcLifetime } from './pi-rpc-lifetime';
import { PiRpcEvents } from './pi-rpc-events';
import { PiRpcExtensionUi } from './pi-rpc-extension-ui';
import {
  READY_REQUEST_ID,
  PROMPT_REQUEST_ID,
  validateReadinessResponse,
  validatePromptResponse,
} from './pi-rpc-responses';
import { asProviderError, deferred } from './pi-rpc-async';

export type { ChildProcessWithoutNullStreams, SpawnOptionsWithoutStdio } from 'node:child_process';

export {
  PI_RPC_PROTOCOL_VERSION,
  PI_RPC_SUPPORTED_VERSIONS,
  PI_RPC_REQUIRED_SAFETY_FLAGS,
  createPiRpcArguments,
  assertRpcCompatibility,
} from './pi-rpc-config';
export {
  PiRpcTimingStage,
  type PiRpcExpectedState,
  type PiRpcPrewarmOptions,
  type PiRpcPromptResult,
  type TerminatePiRpcTree,
} from './pi-rpc-types';

type RuntimeState =
  | 'awaiting-readiness'
  | 'ready-pending'
  | 'ready'
  | 'awaiting-prompt-response'
  | 'running'
  | 'settled-pending'
  | 'completed'
  | 'terminating'
  | 'retired';

/** A prewarmed Pi RPC process that accepts exactly one prompt. */
export class PiRpcOperation {
  readonly #processRetirement: PiRpcRetirement;
  readonly #transport: PiRpcTransport;
  readonly #expected: Readonly<PiRpcExpectedState>;
  readonly #onTiming: ((stage: PiRpcTimingStage) => void) | undefined;
  readonly #ready = deferred<undefined>();
  readonly #completion = deferred<PiRpcPromptResult>();
  readonly #retirement = deferred<undefined>();
  readonly #cleanup = deferred<undefined>();
  readonly #extensionUi: PiRpcExtensionUi;
  readonly #events: PiRpcEvents;
  readonly #lifetime: PiRpcLifetime;
  #state: RuntimeState = 'awaiting-readiness';
  #promptCalled = false;
  #promptCommitted = false;
  #retirementTask: Promise<void> | null = null;
  #successfulRetirement = false;
  #retirementError: ProviderError | null = null;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    options: PiRpcPrewarmOptions,
    validated: ReturnType<typeof validatePrewarmOptions>,
  ) {
    this.#expected = validated.expected;
    this.#events = new PiRpcEvents(
      validated.expected,
      (stage) => this.#timing(stage),
      () => {
        this.#state = 'settled-pending';
      },
      (error) => this.#beginFailure(error),
    );
    this.#extensionUi = new PiRpcExtensionUi((command) => {
      void this.#transport
        .write(command)
        .catch((error: unknown) => this.#beginFailure(asProviderError(error, 'PI_LAUNCH_FAILED')));
    });
    this.#onTiming = options.onTiming;
    // These promises can outlive a caller that aborts before receiving the operation.
    void this.#ready.promise.catch(() => undefined);
    void this.#completion.promise.catch(() => undefined);
    void this.#retirement.promise.catch(() => undefined);
    void this.#cleanup.promise.catch(() => undefined);
    this.#transport = new PiRpcTransport(child, {
      ...(options.limits === undefined ? {} : { limits: options.limits }),
      onRecords: (records) => this.#acceptRecords(records),
      onFault: (error) => this.#handleTransportFault(error),
      onStdoutEnd: () => this.#beginFailure(new ProviderError('PI_LAUNCH_FAILED')),
      onClose: () => {
        this.#processRetirement.observeClose();
        this.#beginFailure(new ProviderError('PI_LAUNCH_FAILED'));
      },
    });
    this.#processRetirement = new PiRpcRetirement(child, this.#transport, options, validated);
    this.#lifetime = new PiRpcLifetime(options.signal, validated.timeoutMs, (error) =>
      this.#beginFailure(error),
    );
    this.#timing(PiRpcTimingStage.ProcessSpawned);
    if (options.signal?.aborted === true) {
      this.#beginFailure(new ProviderError('CANCELLED'));
      return;
    }
    this.#lifetime.listen();
    void this.#transport.write({ id: READY_REQUEST_ID, type: 'get_state' }).then(
      () => this.#timing(PiRpcTimingStage.ReadinessProbeWritten),
      (error: unknown) => this.#beginFailure(asProviderError(error, 'PI_LAUNCH_FAILED')),
    );
  }

  static async start(options: PiRpcPrewarmOptions): Promise<PiRpcOperation> {
    const validated = validatePrewarmOptions(options);
    if (options.signal?.aborted === true) throw new ProviderError('CANCELLED');
    const operation = new PiRpcOperation(spawnPiRpc(options, validated, spawn), options, validated);
    await operation.#ready.promise;
    return operation;
  }

  /** The same bounded retirement promise returned with a successful prompt result. */
  get retirement(): Promise<void> {
    return this.#retirement.promise;
  }

  /** Resolves only after graceful exit or confirmed process-tree termination. */
  get cleanup(): Promise<void> {
    return this.#cleanup.promise;
  }

  async prompt(
    message: string,
    signal?: AbortSignal,
    commit?: () => boolean,
  ): Promise<PiRpcPromptResult> {
    if (this.#promptCalled) throw new ProviderError('INVALID_CONFIG');
    if (this.#state !== 'ready') {
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: !this.#promptCommitted });
    }
    this.#promptCalled = true;
    if (signal?.aborted === true) {
      this.#beginFailure(new ProviderError('CANCELLED'));
      return await this.#completion.promise;
    }
    if (
      message.length === 0 ||
      message.length > MAX_PROVIDER_INPUT_UTF8_BYTES ||
      Buffer.byteLength(message, 'utf8') > MAX_PROVIDER_INPUT_UTF8_BYTES
    ) {
      this.#beginFailure(new ProviderError('REQUEST_TOO_LARGE'));
      return await this.#completion.promise;
    }
    this.#state = 'awaiting-prompt-response';
    this.#lifetime.listenPrompt(signal);
    const command: PiRpcOutboundCommand = {
      id: PROMPT_REQUEST_ID,
      type: 'prompt',
      message,
    };
    this.#events.promptText = message;
    if (commit?.() === false) {
      this.#beginFailure(new ProviderError('UNAVAILABLE'));
      return await this.#completion.promise;
    }
    // From this point onward delivery is ambiguous: this operation never writes another prompt and
    // callers must not retry it automatically.
    this.#promptCommitted = true;
    this.#timing(PiRpcTimingStage.PromptWritten);
    void this.#transport
      .write(command)
      .catch((error: unknown) => this.#beginFailure(asProviderError(error, 'PI_LAUNCH_FAILED')));
    return await this.#completion.promise;
  }

  async abort(): Promise<void> {
    this.#beginFailure(new ProviderError('CANCELLED'));
    await (this.#retirementTask ?? Promise.resolve());
    await this.#retirement.promise;
  }

  #acceptRecords(records: readonly Readonly<Record<string, unknown>>[]): void {
    for (const record of records) this.#acceptRecord(record);
    if (this.#state === 'settled-pending') this.#sealProtocolCompletion();
  }

  #acceptRecord(record: Readonly<Record<string, unknown>>): void {
    if (this.#state === 'retired' || this.#state === 'completed') return;
    if (this.#state === 'terminating') {
      this.#acceptTerminatingRecord(record);
      return;
    }
    const type = record.type;
    if (type === 'extension_ui_request') {
      this.#extensionUi.accept(record);
      return;
    }
    if (type === 'response') {
      this.#acceptResponse(record);
      return;
    }
    if (typeof type !== 'string') throw new ProviderError('INVALID_RESPONSE');
    if (this.#state !== 'running') throw new ProviderError('INVALID_RESPONSE');
    this.#events.accept(type, record);
  }

  #acceptResponse(record: Readonly<Record<string, unknown>>): void {
    if (this.#state === 'awaiting-readiness') {
      this.#acceptReadinessResponse(record);
      return;
    }
    if (this.#state !== 'awaiting-prompt-response') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (!validatePromptResponse(record)) {
      this.#beginFailure(new ProviderError('REMOTE_FAILURE'));
      return;
    }
    this.#state = 'running';
    this.#timing(PiRpcTimingStage.PromptAccepted);
  }

  #acceptReadinessResponse(record: Readonly<Record<string, unknown>>): void {
    validateReadinessResponse(this.#expected, record);
    this.#state = 'ready-pending';
    this.#lifetime.deferReady(() => {
      if (this.#state !== 'ready-pending') return;
      this.#state = 'ready';
      this.#timing(PiRpcTimingStage.Ready);
      this.#ready.resolve(undefined);
    });
  }

  #acceptTerminatingRecord(record: Readonly<Record<string, unknown>>): void {
    if (record.type === 'extension_ui_request') {
      try {
        this.#extensionUi.accept(record);
      } catch {
        // The original terminal reason remains authoritative while cleanup is in progress.
      }
      return;
    }
    this.#processRetirement.acceptResponse(record);
  }

  #sealProtocolCompletion(): void {
    if (this.#state !== 'settled-pending' || this.#events.candidate === null) return;
    this.#state = 'completed';
    this.#successfulRetirement = true;
    this.#transport.sealProtocol();
    this.#lifetime.clear();
    const result = Object.freeze({
      text: this.#events.candidate.text,
      retirement: this.#retirement.promise,
    });
    this.#completion.resolve(result);
    setImmediate(() => this.#beginSuccessfulRetirement());
  }

  #beginSuccessfulRetirement(): void {
    if (this.#state !== 'completed' || this.#retirementTask !== null) return;
    this.#state = 'terminating';
    this.#timing(PiRpcTimingStage.RetirementStarted);
    this.#startRetirement();
  }

  #beginFailure(error: ProviderError): void {
    const completionError = new ProviderError(error.code, {
      fallbackEligible: error.fallbackEligible || !this.#promptCommitted,
    });
    if (
      this.#retirementTask !== null ||
      this.#state === 'retired' ||
      this.#state === 'completed' ||
      this.#state === 'terminating'
    ) {
      return;
    }
    this.#state = 'terminating';
    this.#lifetime.clear();
    this.#startRetirement(completionError);
  }

  #startRetirement(completionError?: ProviderError): void {
    this.#retirementTask = this.#processRetirement.run(completionError !== undefined).then(
      () => {
        this.#cleanup.resolve(undefined);
        this.#finishRetirement();
        if (completionError !== undefined) {
          this.#ready.reject(completionError);
          this.#completion.reject(completionError);
        }
      },
      () => {
        const cleanupError = new ProviderError('PI_LAUNCH_FAILED');
        this.#state = 'retired';
        this.#cleanup.reject(cleanupError);
        this.#retirement.reject(cleanupError);
        if (completionError !== undefined) {
          this.#ready.reject(cleanupError);
          this.#completion.reject(cleanupError);
        }
      },
    );
  }

  #finishRetirement(): void {
    if (this.#state === 'retired') return;
    this.#state = 'retired';
    this.#timing(PiRpcTimingStage.Retired);
    if (this.#retirementError === null) this.#retirement.resolve(undefined);
    else this.#retirement.reject(this.#retirementError);
  }

  #handleTransportFault(error: ProviderError): void {
    if (
      this.#successfulRetirement &&
      (this.#state === 'completed' || this.#state === 'terminating')
    ) {
      this.#retirementError ??= error;
      this.#processRetirement.escalate();
      if (this.#retirementTask === null) this.#beginSuccessfulRetirement();
      return;
    }
    this.#beginFailure(error);
  }

  #timing(stage: PiRpcTimingStage): void {
    try {
      this.#onTiming?.(stage);
    } catch {
      // Timing observers are non-authoritative and receive no operation data.
    }
  }
}

export async function prewarmPiRpcOperation(options: PiRpcPrewarmOptions): Promise<PiRpcOperation> {
  return await PiRpcOperation.start(options);
}
