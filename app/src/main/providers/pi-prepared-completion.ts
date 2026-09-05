import {
  ProviderCompletionRequestSchema,
  type ProviderCompletionRequest,
} from '../../shared/schemas/providers';
import type { PreparedProviderCompletion } from './contracts';
import { ProviderError } from './errors';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import type { PiCliIdentity } from './pi-executable';
import {
  type PiExtensionResolver,
  sameExtensionIdentity,
  canonicalExtensionArguments,
  type ResolvedPiExtensions,
} from './pi-extension-resolver';
import { splitPiModelId, MAX_PI_STDOUT_BYTES, type PiConfig } from './pi-models';
import type { PiSpeculativeOperationPermit } from './pi-operation-scheduler';
import { runPiInvocation } from './pi-process-runtime';
import type { PiProviderOptions } from './pi-provider-options';
import {
  type PiProviderRuntime,
  DEFAULT_PI_TIMEOUT_MS as DEFAULT_TIMEOUT_MS,
  PI_MIN_OPERATION_TIMEOUT_MS,
  classifyPiFailure,
  waitForPiAbort,
} from './pi-provider-runtime';
import {
  assertRpcCompatibility,
  prewarmPiRpcOperation,
  type PiRpcOperation,
  type PiRpcTimingStage,
  type TerminatePiRpcTree,
} from './pi-rpc-operation';

export const PI_RPC_READY_TTL_MS = 15_000;
const MAX_STDOUT_BYTES = MAX_PI_STDOUT_BYTES;
const MAX_STDERR_BYTES = 16 * 1024;
interface FrozenPiLaunch {
  readonly config: PiConfig;
  readonly configuredPath: string | null;
  readonly identity: PiCliIdentity;
  readonly extensions: ResolvedPiExtensions;
  readonly expected: Readonly<{ provider: string; model: string; thinking: PiConfig['thinking'] }>;
  readonly timeoutMs: number;
}

/** Owns single-use leases; the runtime scheduler remains shared with foreground calls. */
export class PiPreparedCompletionFactory {
  readonly #runtime: PiProviderRuntime;
  readonly #extensions: PiExtensionResolver;
  readonly #rpcReadyTtlMs: number;
  readonly #prewarmRpcOperation: typeof prewarmPiRpcOperation;
  readonly #terminateRpcTree: TerminatePiRpcTree | undefined;
  readonly #onRpcTiming: ((stage: PiRpcTimingStage) => void) | undefined;

  constructor(
    options: PiProviderOptions,
    runtime: PiProviderRuntime,
    extensions: PiExtensionResolver,
  ) {
    this.#runtime = runtime;
    this.#extensions = extensions;
    this.#rpcReadyTtlMs = boundedPiDuration(options.rpcReadyTtlMs ?? PI_RPC_READY_TTL_MS);
    this.#prewarmRpcOperation = options.prewarmRpcOperation ?? prewarmPiRpcOperation;
    this.#terminateRpcTree = options.terminateRpcTree;
    this.#onRpcTiming = options.onRpcTiming;
  }

  async prepare(config: PiConfig, signal: AbortSignal): Promise<PreparedProviderCompletion | null> {
    const configuredPath = this.#runtime.configuredPath();
    const initialExtensions = await this.#extensions.resolve(config, signal);
    let identity = await this.#runtime.resolveIdentity(signal, configuredPath);
    const permit = await this.#runtime.scheduler.acquireSpeculative(signal);
    let linked: LinkedAbortSignal | null = null;
    try {
      identity = await this.#runtime.resolvePreparedIdentity(identity, configuredPath, signal);
      const extensions = await this.#extensions.resolve(config, signal);
      if (this.#runtime.configuredPath() !== configuredPath) {
        throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      }
      if (!sameExtensionIdentity(initialExtensions, extensions)) {
        throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      }
      const [provider, model] = splitPiModelId(config.modelId);
      const snapshot: FrozenPiLaunch = Object.freeze({
        config,
        configuredPath,
        identity,
        extensions,
        expected: Object.freeze({ provider, model, thinking: config.thinking }),
        timeoutMs: Math.max(config.timeoutMs ?? DEFAULT_TIMEOUT_MS, PI_MIN_OPERATION_TIMEOUT_MS),
      });
      try {
        assertRpcCompatibility(identity);
      } catch {
        permit.release();
        return this.#createFallbackPreparedCompletion(snapshot);
      }
      const startupLink = linkAbortSignals([signal, permit.revocationSignal]);
      linked = startupLink;
      try {
        const operation = await this.#prewarmRpcOperation({
          identity,
          expected: snapshot.expected,
          explicitExtensions: extensions.canonicalSources,
          signal: startupLink.signal,
          environment: this.#runtime.environment,
          platform: this.#runtime.platform,
          workingDirectory: this.#runtime.workingDirectory,
          ...(this.#runtime.spawnPi === undefined ? {} : { spawnPi: this.#runtime.spawnPi }),
          ...(this.#terminateRpcTree === undefined
            ? {}
            : { terminateTree: this.#terminateRpcTree }),
          timeoutMs: snapshot.timeoutMs,
          ...(this.#onRpcTiming === undefined ? {} : { onTiming: this.#onRpcTiming }),
        });
        linked = null;
        return this.#createRpcPreparedCompletion(snapshot, operation, permit, startupLink);
      } catch (error: unknown) {
        startupLink.dispose();
        linked = null;
        const normalized = toPreparedProviderError(error);
        if (!normalized.fallbackEligible) {
          permit.fail(normalized);
          throw normalized;
        }
        permit.release();
        if (signal.aborted) throw new ProviderError('CANCELLED');
        return this.#createFallbackPreparedCompletion(snapshot);
      }
    } catch (error: unknown) {
      linked?.dispose();
      permit.release();
      throw error;
    }
  }

  #createFallbackPreparedCompletion(snapshot: FrozenPiLaunch): PreparedProviderCompletion {
    const closed = deferred<undefined>();
    let consumed = false;
    let settled = false;
    const timer = setTimeout(() => settle(), this.#rpcReadyTtlMs);
    timer.unref();
    const settle = (): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      closed.resolve(undefined);
    };
    const requestClose = (): void => settle();
    const complete = async (
      requestInput: ProviderCompletionRequest,
      signal: AbortSignal,
    ): Promise<string> => {
      if (consumed) throw new ProviderError('INVALID_CONFIG');
      consumed = true;
      clearTimeout(timer);
      if (settled) throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      let request: ProviderCompletionRequest & { readonly input: string };
      try {
        request = this.#preparedRequest(snapshot, requestInput);
      } catch (error: unknown) {
        settle();
        throw error;
      }
      if (signal.aborted) {
        settle();
        throw new ProviderError('CANCELLED', { fallbackEligible: true });
      }
      try {
        return await this.#runFrozenPrint(snapshot, request.input, signal);
      } finally {
        settle();
      }
    };
    return Object.freeze({ complete, requestClose, closed: closed.promise });
  }

  #createRpcPreparedCompletion(
    snapshot: FrozenPiLaunch,
    operation: PiRpcOperation,
    permit: PiSpeculativeOperationPermit,
    lifetimeLink: LinkedAbortSignal,
  ): PreparedProviderCompletion {
    const closed = deferred<undefined>();
    const closeController = new AbortController();
    let consumed = false;
    let terminal = false;
    let settled = false;
    let cleanupSettled = false;
    let cleanupError: ProviderError | null = null;
    let retirementSettled = false;
    let retirementError: ProviderError | null = null;
    const timer = setTimeout(() => requestClose(), this.#rpcReadyTtlMs);
    timer.unref();

    const settleIfTerminal = (): void => {
      if (settled || !terminal || !cleanupSettled || !retirementSettled) return;
      settled = true;
      clearTimeout(timer);
      lifetimeLink.dispose();
      const error = cleanupError ?? retirementError;
      if (error === null) closed.resolve(undefined);
      else closed.reject(error);
    };
    void operation.cleanup.then(
      () => {
        cleanupSettled = true;
        permit.release();
        settleIfTerminal();
      },
      (error: unknown) => {
        cleanupSettled = true;
        cleanupError = toPreparedProviderError(error);
        permit.fail(cleanupError);
        settleIfTerminal();
      },
    );
    void operation.retirement.then(
      () => {
        retirementSettled = true;
        settleIfTerminal();
      },
      (error: unknown) => {
        retirementSettled = true;
        retirementError = toPreparedProviderError(error);
        settleIfTerminal();
      },
    );

    const requestClose = (): void => {
      if (!terminal) terminal = true;
      clearTimeout(timer);
      if (!closeController.signal.aborted) closeController.abort();
      void operation.abort().catch(() => undefined);
      settleIfTerminal();
    };
    const complete = async (
      requestInput: ProviderCompletionRequest,
      signal: AbortSignal,
    ): Promise<string> => {
      if (consumed) throw new ProviderError('INVALID_CONFIG');
      consumed = true;
      clearTimeout(timer);
      if (terminal) throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
      let request: ProviderCompletionRequest & { readonly input: string };
      try {
        request = this.#preparedRequest(snapshot, requestInput);
      } catch (error: unknown) {
        requestClose();
        throw error;
      }
      if (signal.aborted) {
        requestClose();
        throw new ProviderError('CANCELLED', { fallbackEligible: true });
      }
      const completionLink = linkAbortSignals([signal, closeController.signal]);
      try {
        if (!retirementSettled) {
          try {
            await this.#assertFrozenLaunchCurrent(snapshot, completionLink.signal);
          } catch (error: unknown) {
            terminal = true;
            void operation.abort().catch(() => undefined);
            throw toResourceChangeError(error, signal);
          }
          try {
            const result = await operation.prompt(request.input, completionLink.signal, () =>
              permit.commit(),
            );
            terminal = true;
            settleIfTerminal();
            return result.text;
          } catch (error: unknown) {
            const normalized = toPreparedProviderError(error);
            if (!normalized.fallbackEligible || completionLink.signal.aborted) {
              terminal = true;
              settleIfTerminal();
              throw normalized;
            }
          }
        }
        terminal = true;
        await operation.cleanup;
        return await this.#runFrozenPrint(snapshot, request.input, signal);
      } finally {
        completionLink.dispose();
        settleIfTerminal();
      }
    };
    return Object.freeze({ complete, requestClose, closed: closed.promise });
  }

  #preparedRequest(
    snapshot: FrozenPiLaunch,
    requestInput: ProviderCompletionRequest,
  ): ProviderCompletionRequest & { readonly input: string } {
    let request: ProviderCompletionRequest;
    try {
      request = ProviderCompletionRequestSchema.parse(requestInput);
    } catch {
      throw new ProviderError('INVALID_CONFIG', { fallbackEligible: true });
    }
    if (
      request.image !== undefined ||
      (request.modelId ?? snapshot.config.modelId) !== snapshot.config.modelId
    ) {
      throw new ProviderError('INVALID_CONFIG', { fallbackEligible: true });
    }
    return request;
  }

  async #assertFrozenLaunchCurrent(snapshot: FrozenPiLaunch, signal: AbortSignal): Promise<void> {
    if (this.#runtime.configuredPath() !== snapshot.configuredPath) {
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
    try {
      await waitForPiAbort(this.#runtime.revalidateCli(snapshot.identity, signal), signal);
      const extensions = await this.#extensions.resolve(snapshot.config, signal);
      if (!sameExtensionIdentity(snapshot.extensions, extensions))
        throw new Error('resource changed');
    } catch (error: unknown) {
      if (error instanceof ProviderError && error.code === 'CANCELLED') throw error;
      this.#runtime.invalidateIdentity();
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
  }

  async #runFrozenPrint(
    snapshot: FrozenPiLaunch,
    input: string,
    signal: AbortSignal,
  ): Promise<string> {
    const permit = await this.#runtime.scheduler.acquireForeground(signal);
    try {
      await this.#assertFrozenLaunchCurrent(snapshot, signal);
      const result = await runPiInvocation(
        snapshot.identity.canonicalPath,
        [
          '-p',
          '--model',
          snapshot.config.modelId,
          '--thinking',
          snapshot.config.thinking,
          ...snapshot.identity.safetyFlags,
          ...canonicalExtensionArguments(snapshot.extensions.canonicalSources),
        ],
        input,
        signal,
        snapshot.timeoutMs,
        {
          spawnPi: this.#runtime.spawnPi,
          environment: this.#runtime.environment,
          platform: this.#runtime.platform,
          workingDirectory: this.#runtime.workingDirectory,
          maxStdoutBytes: MAX_STDOUT_BYTES,
          maxStderrBytes: MAX_STDERR_BYTES,
        },
      );
      if (result.code !== 0) throw classifyPiFailure(result.stderr);
      if (
        result.stdout.length > MAX_NATIVE_OUTPUT_CHARACTERS ||
        result.stdout.trim().length === 0
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return result.stdout.trim();
    } finally {
      permit.release();
    }
  }
}

function boundedPiDuration(value: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > DEFAULT_TIMEOUT_MS) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}

function toPreparedProviderError(error: unknown): ProviderError {
  return error instanceof ProviderError ? error : new ProviderError('UNAVAILABLE');
}

function toResourceChangeError(error: unknown, signal: AbortSignal): ProviderError {
  const normalized = toPreparedProviderError(error);
  if (signal.aborted || normalized.code === 'CANCELLED') {
    return new ProviderError('CANCELLED', { fallbackEligible: true });
  }
  return new ProviderError(normalized.code, { fallbackEligible: true });
}

interface LinkedAbortSignal {
  readonly signal: AbortSignal;
  dispose(): void;
}

function linkAbortSignals(signals: readonly AbortSignal[]): LinkedAbortSignal {
  const controller = new AbortController();
  const abort = (): void => {
    if (!controller.signal.aborted) controller.abort();
  };
  for (const signal of signals) {
    if (signal.aborted) abort();
    else signal.addEventListener('abort', abort, { once: true });
  }
  return Object.freeze({
    signal: controller.signal,
    dispose: () => {
      for (const signal of signals) signal.removeEventListener('abort', abort);
    },
  });
}

function deferred<Result>() {
  let resolveDeferred!: (value: Result | PromiseLike<Result>) => void;
  let rejectDeferred!: (reason?: unknown) => void;
  const promise = new Promise<Result>((resolvePromise, rejectPromise) => {
    resolveDeferred = resolvePromise;
    rejectDeferred = rejectPromise;
  });
  return Object.freeze({ promise, resolve: resolveDeferred, reject: rejectDeferred });
}
