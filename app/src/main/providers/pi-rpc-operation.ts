import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { isDeepStrictEqual } from 'node:util';
import {
  MAX_PROVIDER_INPUT_UTF8_BYTES,
  type PiThinkingLevel,
} from '../../shared/schemas/providers';
import { MAX_NATIVE_OUTPUT_CHARACTERS } from './native-common';
import { ProviderError } from './errors';
import type { PiCliIdentity } from './pi-executable';
import { piSpawnCommand, terminateProcessTree, type SpawnPi } from './pi-process-runtime';
import {
  PiRpcTransport,
  validatePiRpcLimits,
  type PiRpcLimits,
  type PiRpcOutboundCommand,
} from './pi-rpc-transport';

export const PI_RPC_PROTOCOL_VERSION = '0.84.3';
export const PI_RPC_SUPPORTED_VERSIONS = Object.freeze(['0.84.2', '0.84.3'] as const);
export const PI_RPC_REQUIRED_SAFETY_FLAGS = Object.freeze([
  '--no-tools',
  '--no-extensions',
  '--no-session',
  '--no-context-files',
  '--no-approve',
  '--no-skills',
  '--no-prompt-templates',
  '--no-themes',
  '--offline',
] as const);

const READY_REQUEST_ID = 'talking-quill-state-1';
const PROMPT_REQUEST_ID = 'talking-quill-prompt-1';
const ABORT_REQUEST_ID = 'talking-quill-abort-1';
const MAX_EXTENSION_UI_REQUESTS = 128;
const DEFAULT_OPERATION_TIMEOUT_MS = 120_000;
const DEFAULT_ABORT_GRACE_MS = 250;
const DEFAULT_RETIREMENT_GRACE_MS = 500;
const DEFAULT_TREE_TERMINATION_TIMEOUT_MS = 5_000;
const MAX_EXTENSION_TEXT = 512 * 1024;
const EXPECTED_ID = /^[A-Za-z0-9][A-Za-z0-9._:@+/-]{0,511}$/u;

export enum PiRpcTimingStage {
  ProcessSpawned = 'process-spawned',
  ReadinessProbeWritten = 'readiness-probe-written',
  Ready = 'ready',
  PromptWritten = 'prompt-written',
  PromptAccepted = 'prompt-accepted',
  AssistantMessageEnded = 'assistant-message-ended',
  AgentSettled = 'agent-settled',
  RetirementStarted = 'retirement-started',
  Retired = 'retired',
}

export interface PiRpcExpectedState {
  readonly provider: string;
  readonly model: string;
  readonly thinking: PiThinkingLevel;
}

export interface PiRpcPromptResult {
  readonly text: string;
  /** Resolves after graceful exit or confirmed process-tree termination. */
  readonly retirement: Promise<void>;
}

export type TerminatePiRpcTree = (
  child: ChildProcessWithoutNullStreams,
  platform: NodeJS.Platform,
  environment: NodeJS.ProcessEnv,
) => Promise<void>;

export interface PiRpcPrewarmOptions {
  readonly identity: PiCliIdentity;
  readonly expected: PiRpcExpectedState;
  /** Canonical extension files or package roots already approved by the later argv layer. */
  readonly explicitExtensions?: readonly string[];
  readonly signal?: AbortSignal;
  readonly environment?: NodeJS.ProcessEnv;
  readonly platform?: NodeJS.Platform;
  readonly workingDirectory?: string;
  readonly spawnPi?: SpawnPi;
  readonly terminateTree?: TerminatePiRpcTree;
  readonly limits?: Partial<PiRpcLimits>;
  readonly timeoutMs?: number;
  readonly abortGraceMs?: number;
  readonly retirementGraceMs?: number;
  readonly treeTerminationTimeoutMs?: number;
  /** Receives enum values only. The caller owns timestamps and any aggregation. */
  readonly onTiming?: (stage: PiRpcTimingStage) => void;
}

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

interface AssistantCandidate {
  readonly text: string;
}

/** A prewarmed Pi RPC process that accepts exactly one prompt. */
export class PiRpcOperation {
  readonly #child: ChildProcessWithoutNullStreams;
  readonly #transport: PiRpcTransport;
  readonly #expected: Readonly<PiRpcExpectedState>;
  readonly #environment: NodeJS.ProcessEnv;
  readonly #platform: NodeJS.Platform;
  readonly #terminateTree: TerminatePiRpcTree;
  readonly #onTiming: ((stage: PiRpcTimingStage) => void) | undefined;
  readonly #abortGraceMs: number;
  readonly #retirementGraceMs: number;
  readonly #treeTerminationTimeoutMs: number;
  readonly #ready = deferred<undefined>();
  readonly #completion = deferred<PiRpcPromptResult>();
  readonly #retirement = deferred<undefined>();
  readonly #cleanup = deferred<undefined>();
  readonly #closed = deferred<undefined>();
  readonly #abortResponse = deferred<undefined>();
  readonly #retirementEscalation = deferred<undefined>();
  readonly #extensionUiIds = new Set<string>();
  readonly #lifetimeSignal: AbortSignal | undefined;
  readonly #lifetimeAbort: () => void;
  #state: RuntimeState = 'awaiting-readiness';
  #timeout: NodeJS.Timeout | null = null;
  #readyTimer: NodeJS.Timeout | null = null;
  #promptSignal: AbortSignal | undefined;
  #promptAbort: (() => void) | undefined;
  #promptCalled = false;
  #promptCommitted = false;
  #promptAccepted = false;
  #promptText: string | null = null;
  #abortWritten = false;
  #abortResponded = false;
  #closedObserved = false;
  #stdoutEnded = false;
  #agentActive = false;
  #agentRuns = 0;
  #turnActive = false;
  #openMessageRole: 'user' | 'assistant' | null = null;
  #openUserMessageWire: string | null = null;
  #lastAssistantMessage: Readonly<Record<string, unknown>> | null = null;
  #assistantEndedInTurn = false;
  #lastAgentEnded = false;
  #sawRetry = false;
  #candidate: AssistantCandidate | null = null;
  #failureTask: Promise<void> | null = null;
  #retirementTask: Promise<void> | null = null;
  #successfulRetirement = false;
  #retirementError: ProviderError | null = null;

  private constructor(
    child: ChildProcessWithoutNullStreams,
    options: PiRpcPrewarmOptions,
    expected: Readonly<PiRpcExpectedState>,
    platform: NodeJS.Platform,
    environment: NodeJS.ProcessEnv,
    timeoutMs: number,
    abortGraceMs: number,
    retirementGraceMs: number,
    treeTerminationTimeoutMs: number,
  ) {
    this.#child = child;
    this.#expected = expected;
    this.#platform = platform;
    this.#environment = environment;
    this.#terminateTree = options.terminateTree ?? terminateProcessTree;
    this.#onTiming = options.onTiming;
    this.#abortGraceMs = abortGraceMs;
    this.#retirementGraceMs = retirementGraceMs;
    this.#treeTerminationTimeoutMs = treeTerminationTimeoutMs;
    this.#lifetimeSignal = options.signal;
    this.#lifetimeAbort = () => this.#beginFailure(new ProviderError('CANCELLED'));
    // These promises can outlive a caller that aborts before receiving the operation.
    void this.#ready.promise.catch(() => undefined);
    void this.#completion.promise.catch(() => undefined);
    void this.#retirement.promise.catch(() => undefined);
    void this.#cleanup.promise.catch(() => undefined);
    void this.#abortResponse.promise.catch(() => undefined);
    this.#transport = new PiRpcTransport(child, {
      ...(options.limits === undefined ? {} : { limits: options.limits }),
      onRecords: (records) => this.#acceptRecords(records),
      onFault: (error) => this.#handleTransportFault(error),
      onStdoutEnd: () => this.#handleStdoutEnd(),
      onClose: () => this.#handleClose(),
    });
    this.#timeout = setTimeout(() => this.#beginFailure(new ProviderError('TIMEOUT')), timeoutMs);
    this.#timeout.unref();
    this.#timing(PiRpcTimingStage.ProcessSpawned);
    if (options.signal?.aborted === true) {
      this.#beginFailure(new ProviderError('CANCELLED'));
      return;
    }
    options.signal?.addEventListener('abort', this.#lifetimeAbort, { once: true });
    void this.#transport.write({ id: READY_REQUEST_ID, type: 'get_state' }).then(
      () => this.#timing(PiRpcTimingStage.ReadinessProbeWritten),
      (error: unknown) => this.#beginFailure(asProviderError(error, 'PI_LAUNCH_FAILED')),
    );
  }

  static async start(options: PiRpcPrewarmOptions): Promise<PiRpcOperation> {
    const validated = validatePrewarmOptions(options);
    if (options.signal?.aborted === true) throw new ProviderError('CANCELLED');
    const command = piSpawnCommand(
      options.identity.canonicalPath,
      validated.args,
      validated.environment,
      validated.platform,
    );
    let child: ChildProcessWithoutNullStreams;
    try {
      child = (options.spawnPi ?? spawn)(command.executable, command.args, {
        env: validated.environment,
        cwd: options.workingDirectory ?? process.cwd(),
        shell: false,
        windowsHide: true,
        windowsVerbatimArguments:
          validated.platform === 'win32' && /\.(?:cmd|bat)$/iu.test(options.identity.canonicalPath),
        detached: validated.platform !== 'win32',
        stdio: ['pipe', 'pipe', 'pipe'],
      } satisfies SpawnOptionsWithoutStdio);
    } catch {
      throw new ProviderError('PI_LAUNCH_FAILED', { fallbackEligible: true });
    }
    const operation = new PiRpcOperation(
      child,
      options,
      validated.expected,
      validated.platform,
      validated.environment,
      validated.timeoutMs,
      validated.abortGraceMs,
      validated.retirementGraceMs,
      validated.treeTerminationTimeoutMs,
    );
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
    this.#promptSignal = signal;
    this.#promptAbort = () => this.#beginFailure(new ProviderError('CANCELLED'));
    signal?.addEventListener('abort', this.#promptAbort, { once: true });
    const command: PiRpcOutboundCommand = {
      id: PROMPT_REQUEST_ID,
      type: 'prompt',
      message,
    };
    this.#promptText = message;
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
    await (this.#failureTask ?? this.#retirementTask ?? Promise.resolve());
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
      this.#acceptExtensionUiRequest(record);
      return;
    }
    if (type === 'response') {
      this.#acceptResponse(record);
      return;
    }
    if (typeof type !== 'string') throw new ProviderError('INVALID_RESPONSE');
    this.#acceptEvent(type, record);
  }

  #acceptResponse(record: Readonly<Record<string, unknown>>): void {
    if (this.#state === 'awaiting-readiness') {
      this.#acceptReadinessResponse(record);
      return;
    }
    if (record.id !== PROMPT_REQUEST_ID || record.command !== 'prompt') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (this.#state !== 'awaiting-prompt-response') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (record.success !== true || !exactKeys(record, ['command', 'id', 'success', 'type'])) {
      if (
        record.success === false &&
        exactKeys(record, ['command', 'error', 'id', 'success', 'type']) &&
        validText(record.error, 16_384, true)
      ) {
        this.#beginFailure(new ProviderError('REMOTE_FAILURE'));
        return;
      }
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#promptAccepted = true;
    this.#state = 'running';
    this.#timing(PiRpcTimingStage.PromptAccepted);
  }

  #acceptReadinessResponse(record: Readonly<Record<string, unknown>>): void {
    if (
      record.id !== READY_REQUEST_ID ||
      record.command !== 'get_state' ||
      record.success !== true ||
      !exactKeys(record, ['command', 'data', 'id', 'success', 'type'])
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const data = requireRecord(record.data);
    if (
      !onlyKeys(data, [
        'autoCompactionEnabled',
        'followUpMode',
        'isCompacting',
        'isStreaming',
        'messageCount',
        'model',
        'pendingMessageCount',
        'sessionFile',
        'sessionId',
        'sessionName',
        'steeringMode',
        'thinkingLevel',
      ]) ||
      hasOwn(data, 'sessionFile') ||
      hasOwn(data, 'sessionName') ||
      data.isStreaming !== false ||
      data.isCompacting !== false ||
      data.messageCount !== 0 ||
      data.pendingMessageCount !== 0 ||
      data.thinkingLevel !== this.#expected.thinking ||
      !['all', 'one-at-a-time'].includes(String(data.steeringMode)) ||
      !['all', 'one-at-a-time'].includes(String(data.followUpMode)) ||
      typeof data.autoCompactionEnabled !== 'boolean' ||
      !validText(data.sessionId, 256, false)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const model = requireRecord(data.model);
    if (model.provider !== this.#expected.provider || model.id !== this.#expected.model) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#state = 'ready-pending';
    this.#readyTimer = setTimeout(() => {
      this.#readyTimer = null;
      if (this.#state !== 'ready-pending') return;
      this.#state = 'ready';
      this.#timing(PiRpcTimingStage.Ready);
      this.#ready.resolve(undefined);
    }, 0);
  }

  #acceptEvent(type: string, record: Readonly<Record<string, unknown>>): void {
    if (!this.#promptAccepted || (this.#state !== 'running' && this.#state !== 'settled-pending')) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (this.#state === 'settled-pending') throw new ProviderError('INVALID_RESPONSE');
    switch (type) {
      case 'agent_start':
        this.#acceptAgentStart(record);
        return;
      case 'turn_start':
        this.#acceptTurnStart(record);
        return;
      case 'message_start':
        this.#acceptMessageStart(record);
        return;
      case 'message_update':
        this.#acceptMessageUpdate(record);
        return;
      case 'message_end':
        this.#acceptMessageEnd(record);
        return;
      case 'turn_end':
        this.#acceptTurnEnd(record);
        return;
      case 'agent_end':
        this.#acceptAgentEnd(record);
        return;
      case 'auto_retry_start':
        this.#acceptAutoRetryStart(record);
        return;
      case 'auto_retry_end':
        this.#acceptAutoRetryEnd(record);
        return;
      case 'agent_settled':
        this.#acceptAgentSettled(record);
        return;
      default:
        throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptAgentStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['type']) ||
      this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      (this.#agentRuns > 0 && !this.#lastAgentEnded)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#agentActive = true;
    this.#agentRuns += 1;
    this.#lastAgentEnded = false;
    this.#candidate = null;
  }

  #acceptTurnStart(record: Readonly<Record<string, unknown>>): void {
    if (!exactKeys(record, ['type']) || !this.#agentActive || this.#turnActive) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#turnActive = true;
    this.#assistantEndedInTurn = false;
  }

  #acceptMessageStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== null
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    if (message.role !== 'user' && message.role !== 'assistant') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (message.role === 'assistant') this.#assertAssistantIdentity(message, true);
    else {
      if (userMessageText(message) !== this.#promptText)
        throw new ProviderError('INVALID_RESPONSE');
      this.#openUserMessageWire = JSON.stringify(message);
    }
    this.#openMessageRole = message.role;
  }

  #acceptMessageUpdate(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['assistantMessageEvent', 'type', 'usage']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== 'assistant' ||
      !isRecord(record.usage)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const event = requireRecord(record.assistantMessageEvent);
    const eventType = event.type;
    if (
      ![
        'text_start',
        'text_delta',
        'text_end',
        'thinking_start',
        'thinking_delta',
        'thinking_end',
      ].includes(String(eventType)) ||
      !validIndex(event.contentIndex)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (eventType === 'text_start' || eventType === 'thinking_start') {
      if (!exactKeys(event, ['contentIndex', 'type'])) throw new ProviderError('INVALID_RESPONSE');
      return;
    }
    if (eventType === 'text_delta' || eventType === 'thinking_delta') {
      if (!exactKeys(event, ['contentIndex', 'delta', 'type']) || typeof event.delta !== 'string') {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return;
    }
    if (
      !exactKeys(event, ['content', 'contentIndex', 'type']) ||
      typeof event.content !== 'string'
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptMessageEnd(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole === null
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    if (message.role !== this.#openMessageRole) throw new ProviderError('INVALID_RESPONSE');
    this.#openMessageRole = null;
    if (message.role !== 'assistant') {
      if (userMessageText(message) !== this.#promptText)
        throw new ProviderError('INVALID_RESPONSE');
      if (JSON.stringify(message) !== this.#openUserMessageWire) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      this.#openUserMessageWire = null;
      return;
    }
    if (this.#assistantEndedInTurn) throw new ProviderError('INVALID_RESPONSE');
    this.#assistantEndedInTurn = true;
    this.#assertAssistantIdentity(message, false);
    const stopReason = message.stopReason;
    if (
      !['stop', 'length', 'toolUse', 'error', 'aborted', 'deferred'].includes(String(stopReason))
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (stopReason !== 'stop') validateAssistantBlocks(message, false);
    this.#candidate = stopReason === 'stop' ? { text: finalAssistantText(message) } : null;
    this.#lastAssistantMessage = message;
    this.#timing(PiRpcTimingStage.AssistantMessageEnded);
  }

  #acceptTurnEnd(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['message', 'toolResults', 'type']) ||
      !this.#agentActive ||
      !this.#turnActive ||
      this.#openMessageRole !== null ||
      !this.#assistantEndedInTurn ||
      !Array.isArray(record.toolResults) ||
      record.toolResults.length !== 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const message = requireRecord(record.message);
    this.#assertAssistantIdentity(message, false);
    // Pi repeats the authoritative message as a fresh JSON object. Object key order is not part of
    // the 0.84.2 event schema, but all values and array order must remain identical.
    if (!isDeepStrictEqual(message, this.#lastAssistantMessage)) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#turnActive = false;
  }

  #acceptAgentEnd(record: Readonly<Record<string, unknown>>): void {
    // Pi 0.84.2's AgentEvent shape contains only type and messages. Some session surfaces may
    // append willRetry, so accept a boolean when present without treating it as authoritative.
    const validKeys =
      exactKeys(record, ['messages', 'type']) ||
      (exactKeys(record, ['messages', 'type', 'willRetry']) &&
        typeof record.willRetry === 'boolean');
    if (
      !validKeys ||
      !this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      !Array.isArray(record.messages) ||
      record.messages.length === 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    const messages = record.messages as readonly unknown[];
    let lastAssistant: unknown;
    for (let index = messages.length - 1; index >= 0; index -= 1) {
      const message = messages[index];
      if (isRecord(message) && message.role === 'assistant') {
        lastAssistant = message;
        break;
      }
    }
    if (!isRecord(lastAssistant) || !isDeepStrictEqual(lastAssistant, this.#lastAssistantMessage)) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#agentActive = false;
    this.#lastAgentEnded = true;
  }

  #acceptAutoRetryStart(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['attempt', 'delayMs', 'errorMessage', 'maxAttempts', 'type']) ||
      !this.#lastAgentEnded ||
      !validPositiveInteger(record.attempt, 100) ||
      !validPositiveInteger(record.maxAttempts, 100) ||
      !validPositiveInteger(record.delayMs, 10 * 60_000) ||
      !validText(record.errorMessage, 16_384, true)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#sawRetry = true;
    this.#candidate = null;
  }

  #acceptAutoRetryEnd(record: Readonly<Record<string, unknown>>): void {
    if (!this.#sawRetry || typeof record.success !== 'boolean') {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (record.success) {
      if (
        !exactKeys(record, ['attempt', 'success', 'type']) ||
        !validPositiveInteger(record.attempt, 100)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return;
    }
    if (
      !exactKeys(record, ['attempt', 'finalError', 'success', 'type']) ||
      !validPositiveInteger(record.attempt, 100) ||
      !validText(record.finalError, 16_384, true)
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptAgentSettled(record: Readonly<Record<string, unknown>>): void {
    if (
      !exactKeys(record, ['type']) ||
      this.#agentActive ||
      this.#turnActive ||
      this.#openMessageRole !== null ||
      !this.#lastAgentEnded ||
      this.#agentRuns === 0
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    if (this.#candidate === null) {
      this.#beginFailure(new ProviderError('REMOTE_FAILURE'));
      return;
    }
    this.#state = 'settled-pending';
    this.#timing(PiRpcTimingStage.AgentSettled);
  }

  #assertAssistantIdentity(
    message: Readonly<Record<string, unknown>>,
    allowPending: boolean,
  ): void {
    if (
      message.role !== 'assistant' ||
      message.provider !== this.#expected.provider ||
      message.model !== this.#expected.model ||
      !Array.isArray(message.content) ||
      message.content.length > 256 ||
      !validText(message.api, 128, false) ||
      !isRecord(message.usage) ||
      !Number.isSafeInteger(message.timestamp) ||
      Number(message.timestamp) < 0 ||
      (allowPending && message.stopReason !== 'pending') ||
      (!allowPending && message.stopReason === 'pending')
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
  }

  #acceptExtensionUiRequest(record: Readonly<Record<string, unknown>>): void {
    const method = validateExtensionUiRequest(record);
    const id = record.id as string;
    if (
      this.#extensionUiIds.size >= MAX_EXTENSION_UI_REQUESTS ||
      this.#extensionUiIds.has(id) ||
      id === READY_REQUEST_ID ||
      id === PROMPT_REQUEST_ID ||
      id === ABORT_REQUEST_ID
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    this.#extensionUiIds.add(id);
    if (!['select', 'confirm', 'input', 'editor'].includes(method)) return;
    void this.#transport
      .write({ type: 'extension_ui_response', id, cancelled: true })
      .catch((error: unknown) => this.#beginFailure(asProviderError(error, 'PI_LAUNCH_FAILED')));
  }

  #acceptTerminatingRecord(record: Readonly<Record<string, unknown>>): void {
    if (record.type === 'extension_ui_request') {
      try {
        this.#acceptExtensionUiRequest(record);
      } catch {
        // The original terminal reason remains authoritative while cleanup is in progress.
      }
      return;
    }
    if (
      record.type === 'response' &&
      record.id === ABORT_REQUEST_ID &&
      record.command === 'abort' &&
      record.success === true &&
      exactKeys(record, ['command', 'id', 'success', 'type']) &&
      !this.#abortResponded
    ) {
      this.#abortResponded = true;
      this.#abortResponse.resolve(undefined);
    }
  }

  #sealProtocolCompletion(): void {
    if (this.#state !== 'settled-pending' || this.#candidate === null) return;
    this.#state = 'completed';
    this.#successfulRetirement = true;
    this.#transport.sealProtocol();
    this.#clearOperationListeners();
    const result = Object.freeze({
      text: this.#candidate.text,
      retirement: this.#retirement.promise,
    });
    this.#completion.resolve(result);
    setImmediate(() => this.#beginSuccessfulRetirement());
  }

  #beginSuccessfulRetirement(): void {
    if (this.#state !== 'completed' || this.#retirementTask !== null) return;
    this.#state = 'terminating';
    this.#timing(PiRpcTimingStage.RetirementStarted);
    this.#retirementTask = this.#retire(false).then(
      () => {
        this.#cleanup.resolve(undefined);
        this.#finishRetirement();
      },
      () => {
        const error = new ProviderError('PI_LAUNCH_FAILED');
        this.#state = 'retired';
        this.#cleanup.reject(error);
        this.#retirement.reject(error);
      },
    );
  }

  #beginFailure(error: ProviderError): void {
    const completionError = new ProviderError(error.code, {
      fallbackEligible: error.fallbackEligible || !this.#promptCommitted,
    });
    if (
      this.#failureTask !== null ||
      this.#state === 'retired' ||
      this.#state === 'completed' ||
      this.#state === 'terminating'
    ) {
      return;
    }
    if (this.#readyTimer !== null) {
      clearTimeout(this.#readyTimer);
      this.#readyTimer = null;
    }
    this.#state = 'terminating';
    this.#clearOperationListeners();
    this.#failureTask = this.#retire(true).then(
      () => {
        this.#cleanup.resolve(undefined);
        this.#finishRetirement();
        this.#ready.reject(completionError);
        this.#completion.reject(completionError);
      },
      () => {
        const cleanupError = new ProviderError('PI_LAUNCH_FAILED');
        this.#state = 'retired';
        this.#cleanup.reject(cleanupError);
        this.#retirement.reject(cleanupError);
        this.#ready.reject(cleanupError);
        this.#completion.reject(cleanupError);
      },
    );
  }

  async #retire(cooperativeAbort: boolean): Promise<void> {
    if (cooperativeAbort && !this.#closedObserved && !this.#abortWritten) {
      this.#abortWritten = true;
      const abortWrite = this.#transport
        .write({ id: ABORT_REQUEST_ID, type: 'abort' })
        .catch(() => undefined);
      await Promise.race([
        Promise.all([abortWrite, this.#abortResponse.promise]).then(() => undefined),
        this.#closed.promise,
        delay(this.#abortGraceMs),
      ]);
    }
    if (!this.#closedObserved) {
      this.#transport.end();
      await Promise.race([
        this.#closed.promise,
        this.#retirementEscalation.promise,
        delay(this.#retirementGraceMs),
      ]);
    }
    if (!this.#closedObserved) {
      await Promise.race([
        this.#terminateTree(this.#child, this.#platform, this.#environment),
        delay(this.#treeTerminationTimeoutMs).then(() => {
          throw new ProviderError('PI_LAUNCH_FAILED');
        }),
      ]);
    }
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
      this.#retirementEscalation.resolve(undefined);
      if (this.#retirementTask === null) this.#beginSuccessfulRetirement();
      return;
    }
    this.#beginFailure(error);
  }

  #handleStdoutEnd(): void {
    this.#stdoutEnded = true;
    if (this.#state !== 'terminating' && this.#state !== 'retired') {
      this.#beginFailure(new ProviderError('PI_LAUNCH_FAILED'));
    }
  }

  #handleClose(): void {
    if (!this.#closedObserved) {
      this.#closedObserved = true;
      this.#closed.resolve(undefined);
    }
    if (
      this.#state !== 'terminating' &&
      this.#state !== 'retired' &&
      !(this.#state === 'completed' && this.#stdoutEnded)
    ) {
      this.#beginFailure(new ProviderError('PI_LAUNCH_FAILED'));
    }
  }

  #clearOperationListeners(): void {
    if (this.#timeout !== null) {
      clearTimeout(this.#timeout);
      this.#timeout = null;
    }
    this.#lifetimeSignal?.removeEventListener('abort', this.#lifetimeAbort);
    if (this.#promptAbort !== undefined) {
      this.#promptSignal?.removeEventListener('abort', this.#promptAbort);
      this.#promptAbort = undefined;
    }
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

export function createPiRpcArguments(
  identity: Pick<PiCliIdentity, 'packageVersion' | 'safetyFlags'>,
  expected: PiRpcExpectedState,
  explicitExtensions: readonly string[] = [],
): readonly string[] {
  assertRpcCompatibility(identity);
  const frozen = validateExpectedState(expected);
  if (explicitExtensions.length > 8) throw new ProviderError('INVALID_CONFIG');
  const extensionArgs: string[] = [];
  for (const extension of explicitExtensions) {
    if (
      extension.length === 0 ||
      extension.length > 512 ||
      extension.startsWith('-') ||
      !noControls(extension)
    ) {
      throw new ProviderError('INVALID_CONFIG');
    }
    extensionArgs.push('-e', extension);
  }
  return Object.freeze([
    '--mode',
    'rpc',
    '--provider',
    frozen.provider,
    '--model',
    frozen.model,
    '--thinking',
    frozen.thinking,
    ...identity.safetyFlags,
    ...extensionArgs,
  ]);
}

export function assertRpcCompatibility(
  identity: Pick<PiCliIdentity, 'packageVersion' | 'safetyFlags'>,
): void {
  if (
    !PI_RPC_SUPPORTED_VERSIONS.some((version) => version === identity.packageVersion) ||
    identity.safetyFlags.length !== PI_RPC_REQUIRED_SAFETY_FLAGS.length ||
    !identity.safetyFlags.every((flag, index) => flag === PI_RPC_REQUIRED_SAFETY_FLAGS[index])
  ) {
    throw new ProviderError('PI_INCOMPATIBLE');
  }
}

function validatePrewarmOptions(options: PiRpcPrewarmOptions): {
  readonly args: readonly string[];
  readonly expected: Readonly<PiRpcExpectedState>;
  readonly environment: NodeJS.ProcessEnv;
  readonly platform: NodeJS.Platform;
  readonly timeoutMs: number;
  readonly abortGraceMs: number;
  readonly retirementGraceMs: number;
  readonly treeTerminationTimeoutMs: number;
} {
  const expected = validateExpectedState(options.expected);
  validatePiRpcLimits(options.limits);
  const environment = options.environment ?? process.env;
  const platform = options.platform ?? process.platform;
  const timeoutMs = boundedDuration(options.timeoutMs ?? DEFAULT_OPERATION_TIMEOUT_MS, 600_000);
  const abortGraceMs = boundedDuration(options.abortGraceMs ?? DEFAULT_ABORT_GRACE_MS, 5_000);
  const retirementGraceMs = boundedDuration(
    options.retirementGraceMs ?? DEFAULT_RETIREMENT_GRACE_MS,
    5_000,
  );
  const treeTerminationTimeoutMs = boundedDuration(
    options.treeTerminationTimeoutMs ?? DEFAULT_TREE_TERMINATION_TIMEOUT_MS,
    10_000,
  );
  return Object.freeze({
    args: createPiRpcArguments(options.identity, expected, options.explicitExtensions),
    expected,
    environment,
    platform,
    timeoutMs,
    abortGraceMs,
    retirementGraceMs,
    treeTerminationTimeoutMs,
  });
}

function validateExpectedState(expected: PiRpcExpectedState): Readonly<PiRpcExpectedState> {
  if (
    !EXPECTED_ID.test(expected.provider) ||
    !EXPECTED_ID.test(expected.model) ||
    !['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'].includes(expected.thinking)
  ) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return Object.freeze({
    provider: expected.provider,
    model: expected.model,
    thinking: expected.thinking,
  });
}

function finalAssistantText(message: Readonly<Record<string, unknown>>): string {
  return validateAssistantBlocks(message, true);
}

function validateAssistantBlocks(
  message: Readonly<Record<string, unknown>>,
  requireText: boolean,
): string {
  const content = message.content;
  if (!Array.isArray(content) || content.length > 256 || (requireText && content.length === 0)) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  let text = '';
  for (const rawBlock of content) {
    const block = requireRecord(rawBlock);
    if (block.type === 'thinking') {
      if (
        typeof block.thinking !== 'string' ||
        !optionalKeys(block, ['thinking', 'type'], ['redacted', 'thinkingSignature']) ||
        (block.redacted !== undefined && typeof block.redacted !== 'boolean') ||
        (block.thinkingSignature !== undefined && typeof block.thinkingSignature !== 'string')
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      continue;
    }
    if (
      block.type !== 'text' ||
      typeof block.text !== 'string' ||
      !optionalKeys(block, ['text', 'type'], ['textSignature']) ||
      (block.textSignature !== undefined && typeof block.textSignature !== 'string')
    ) {
      throw new ProviderError('INVALID_RESPONSE');
    }
    text += block.text;
    if (text.length > MAX_NATIVE_OUTPUT_CHARACTERS) {
      throw new ProviderError('RESPONSE_TOO_LARGE');
    }
  }
  const result = text.trim();
  if (requireText && result.length === 0) throw new ProviderError('INVALID_RESPONSE');
  return result;
}

function userMessageText(message: Readonly<Record<string, unknown>>): string {
  if (
    message.role !== 'user' ||
    !Number.isSafeInteger(message.timestamp) ||
    Number(message.timestamp) < 0
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  if (typeof message.content === 'string') return message.content;
  if (!Array.isArray(message.content) || message.content.length !== 1) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  const block = requireRecord(message.content[0]);
  if (
    !exactKeys(block, ['text', 'type']) ||
    block.type !== 'text' ||
    typeof block.text !== 'string'
  ) {
    throw new ProviderError('INVALID_RESPONSE');
  }
  return block.text;
}

function validateExtensionUiRequest(record: Readonly<Record<string, unknown>>): string {
  if (!validId(record.id) || typeof record.method !== 'string') {
    throw new ProviderError('INVALID_RESPONSE');
  }
  switch (record.method) {
    case 'select':
      if (
        !optionalKeys(record, ['id', 'method', 'options', 'title', 'type'], ['timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validStringArray(record.options, 128, 4_096) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'confirm':
      if (
        !optionalKeys(record, ['id', 'message', 'method', 'title', 'type'], ['timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validText(record.message, 16_384, true) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'input':
      if (
        !optionalKeys(record, ['id', 'method', 'title', 'type'], ['placeholder', 'timeout']) ||
        !validText(record.title, 4_096, true) ||
        !validOptionalText(record.placeholder, 4_096) ||
        !validOptionalTimeout(record.timeout)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'editor':
      if (
        !optionalKeys(record, ['id', 'method', 'title', 'type'], ['prefill']) ||
        !validText(record.title, 4_096, true) ||
        !validOptionalText(record.prefill, MAX_EXTENSION_TEXT)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'notify':
      if (
        !optionalKeys(record, ['id', 'message', 'method', 'type'], ['notifyType']) ||
        !validText(record.message, 16_384, true) ||
        (record.notifyType !== undefined &&
          (typeof record.notifyType !== 'string' ||
            !['info', 'warning', 'error'].includes(record.notifyType)))
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setStatus':
      if (
        !optionalKeys(record, ['id', 'method', 'statusKey', 'type'], ['statusText']) ||
        !validText(record.statusKey, 512, false) ||
        !validOptionalText(record.statusText, 16_384)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setWidget':
      if (
        !optionalKeys(
          record,
          ['id', 'method', 'type', 'widgetKey'],
          ['widgetLines', 'widgetPlacement'],
        ) ||
        !validText(record.widgetKey, 512, false) ||
        (record.widgetLines !== undefined && !validStringArray(record.widgetLines, 256, 16_384)) ||
        (record.widgetPlacement !== undefined &&
          (typeof record.widgetPlacement !== 'string' ||
            !['aboveEditor', 'belowEditor'].includes(record.widgetPlacement)))
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'setTitle':
      if (
        !exactKeys(record, ['id', 'method', 'title', 'type']) ||
        !validText(record.title, 4_096, true)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    case 'set_editor_text':
      if (
        !exactKeys(record, ['id', 'method', 'text', 'type']) ||
        !validText(record.text, MAX_EXTENSION_TEXT, true)
      ) {
        throw new ProviderError('INVALID_RESPONSE');
      }
      return record.method;
    default:
      throw new ProviderError('INVALID_RESPONSE');
  }
}

function exactKeys(record: Readonly<Record<string, unknown>>, keys: readonly string[]): boolean {
  const actual = Object.keys(record).sort();
  const expected = [...keys].sort();
  return actual.length === expected.length && actual.every((key, index) => key === expected[index]);
}

function optionalKeys(
  record: Readonly<Record<string, unknown>>,
  required: readonly string[],
  optional: readonly string[],
): boolean {
  const keys = Object.keys(record);
  return (
    required.every((key) => keys.includes(key)) &&
    keys.every((key) => [...required, ...optional].includes(key))
  );
}

function onlyKeys(record: Readonly<Record<string, unknown>>, allowed: readonly string[]): boolean {
  return Object.keys(record).every((key) => allowed.includes(key));
}

function validStringArray(value: unknown, maximumItems: number, maximumLength: number): boolean {
  return (
    Array.isArray(value) &&
    value.length <= maximumItems &&
    value.every((item) => typeof item === 'string' && item.length <= maximumLength)
  );
}

function validOptionalText(value: unknown, maximum: number): boolean {
  return value === undefined || (typeof value === 'string' && value.length <= maximum);
}

function validOptionalTimeout(value: unknown): boolean {
  return value === undefined || validPositiveInteger(value, 10 * 60_000);
}

function validPositiveInteger(value: unknown, maximum: number): value is number {
  return Number.isInteger(value) && Number(value) >= 0 && Number(value) <= maximum;
}

function validIndex(value: unknown): value is number {
  return Number.isInteger(value) && Number(value) >= 0 && Number(value) <= 255;
}

function validText(value: unknown, maximum: number, allowEmpty: boolean): value is string {
  return (
    typeof value === 'string' &&
    value.length <= maximum &&
    (allowEmpty || value.length > 0) &&
    noControlsExceptWhitespace(value)
  );
}

function validId(value: unknown): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= 128 && noControls(value);
}

function noControls(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x20 || code === 0x7f) return false;
  }
  return true;
}

function noControlsExceptWhitespace(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (
      (code < 0x20 && character !== '\n' && character !== '\r' && character !== '\t') ||
      code === 0x7f
    ) {
      return false;
    }
  }
  return true;
}

function requireRecord(value: unknown): Readonly<Record<string, unknown>> {
  if (!isRecord(value)) throw new ProviderError('INVALID_RESPONSE');
  return value;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function hasOwn(record: Readonly<Record<string, unknown>>, key: string): boolean {
  return Object.prototype.hasOwnProperty.call(record, key);
}

function boundedDuration(value: number, maximum: number): number {
  if (!Number.isSafeInteger(value) || value < 1 || value > maximum) {
    throw new ProviderError('INVALID_CONFIG');
  }
  return value;
}

function asProviderError(
  error: unknown,
  fallback: ConstructorParameters<typeof ProviderError>[0],
): ProviderError {
  return error instanceof ProviderError ? error : new ProviderError(fallback);
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => {
    const timer = setTimeout(resolveDelay, milliseconds);
    timer.unref();
  });
}

function deferred<Result>(): {
  readonly promise: Promise<Result>;
  readonly resolve: (result: Result) => void;
  readonly reject: (error: Error) => void;
} {
  let resolve!: (result: Result) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Result>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return Object.freeze({ promise, resolve, reject });
}
