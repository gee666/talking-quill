import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { dirname } from 'node:path';
import { performance } from 'node:perf_hooks';
import {
  HELPER_PROTOCOL_VERSION,
  HelperNotificationSchema,
  HelperRpcResponseSchema,
  helperParamsSchemas,
  helperResultSchemas,
  type ActivationBinding,
  type HelperFrontApp,
  type HelperInitializeResult,
  type HelperMethod,
  type HelperNotification,
  type HelperParams,
  type HelperPasteResult,
  type HelperPermissions,
  type HelperResult,
} from '../../shared/helper/protocol';
import {
  DEFAULT_HELPER_PERMISSIONS,
  HelperReadinessSchema,
  INITIAL_HELPER_READINESS,
  type HelperReadiness,
  type HelperReadinessReason,
} from '../../shared/schemas/helper-readiness';
import { decodeHelperJson, encodeHelperFrame, HelperFrameDecoder } from './framing';
import { HelperBinaryError, type HelperPlatform, validateHelperExecutable } from './helper-path';

const HANDSHAKE_TIMEOUT_MS = 3_000;
const REQUEST_TIMEOUT_MS = 3_000;
const HEARTBEAT_INTERVAL_MS = 5_000;
const SHUTDOWN_TIMEOUT_MS = 1_000;
const STDERR_LIMIT_BYTES = 8 * 1024;
const OUTBOUND_QUEUE_CAPACITY = 256;
const FAILURE_WINDOW_MS = 2 * 60_000;
const FAILURE_LIMIT = 5;
const RESTART_DELAYS_MS = [250, 1_000, 4_000, 15_000, 30_000] as const;

type SpawnHelper = (
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

interface PendingRequest {
  readonly method: HelperMethod;
  readonly resolve: (value: unknown) => void;
  readonly reject: (error: Error) => void;
  readonly deadlineAt: number;
  readonly timeoutReason: HelperReadinessReason;
  readonly removeAbort: () => void;
  readonly onPasteCommitted?: (() => void) | undefined;
  timer: NodeJS.Timeout | null;
  dispatched: boolean;
  abortRequested: boolean;
  pasteCommitted: boolean;
}

interface QueuedWrite {
  readonly id: number;
  readonly frame: Buffer;
  readonly child: ChildProcessWithoutNullStreams;
}

export interface HelperClientOptions {
  readonly executablePath: string;
  readonly expectedHelperVersion: string;
  readonly platform: HelperPlatform;
  readonly architecture: 'x64' | 'arm64';
  readonly spawnHelper?: SpawnHelper;
}

export class HelperClientError extends Error {
  readonly code: 'not-running' | 'request-timeout' | 'rpc-error' | 'transport-error';
  readonly rpcCode: number | null;

  constructor(code: HelperClientError['code'], message: string, rpcCode: number | null = null) {
    super(message);
    this.name = 'HelperClientError';
    this.code = code;
    this.rpcCode = rpcCode;
  }
}

export class HelperClient {
  readonly #options: HelperClientOptions;
  readonly #spawnHelper: SpawnHelper;
  readonly #pending = new Map<number, PendingRequest>();
  readonly #readinessListeners = new Set<(readiness: HelperReadiness) => void>();
  readonly #notificationListeners = new Set<(notification: HelperNotification) => void>();
  readonly #writeQueue: QueuedWrite[] = [];
  #child: ChildProcessWithoutNullStreams | null = null;
  #decoder = new HelperFrameDecoder();
  #writeBlocked = false;
  #writeClosed = true;
  #nextRequestId = 1;
  #generation = 0;
  #desiredRunning = false;
  #desiredActivation: {
    readonly enabled: boolean;
    readonly bindings: readonly ActivationBinding[];
  } = {
    enabled: false,
    bindings: [],
  };
  #launching: Promise<void> | null = null;
  #stopOperation: Promise<void> | null = null;
  #restartTimer: NodeJS.Timeout | null = null;
  #heartbeatTimer: NodeJS.Timeout | null = null;
  #plannedExit: { readonly reason: HelperReadinessReason; readonly restart: boolean } | null = null;
  #failureTimes: number[] = [];
  #readiness: HelperReadiness = INITIAL_HELPER_READINESS;
  #diagnostics = '';

  constructor(options: HelperClientOptions) {
    this.#options = options;
    this.#spawnHelper = options.spawnHelper ?? defaultSpawnHelper;
  }

  get readiness(): HelperReadiness {
    return this.#readiness;
  }

  get diagnostics(): string {
    return this.#diagnostics;
  }

  subscribeReadiness(listener: (readiness: HelperReadiness) => void): () => void {
    this.#readinessListeners.add(listener);
    return () => this.#readinessListeners.delete(listener);
  }

  subscribeNotifications(listener: (notification: HelperNotification) => void): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  async start(): Promise<void> {
    this.#desiredRunning = true;
    const stopping = this.#stopOperation;
    if (stopping !== null) await stopping;
    if (!this.#canLaunch()) return;
    if (this.#launching === null) {
      this.#launching = this.#launch().finally(() => {
        this.#launching = null;
      });
    }
    await this.#launching;
  }

  async restart(): Promise<void> {
    this.#desiredRunning = true;
    const stopping = this.#stopOperation;
    if (stopping !== null) await stopping;
    if (!this.#wantsRunning()) return;
    this.#clearRestart();
    if (this.#child !== null) {
      this.#terminateCurrent('unexpected-exit', true);
      return;
    }
    await this.start();
  }

  async stop(): Promise<void> {
    this.#desiredRunning = false;
    if (this.#stopOperation !== null) return this.#stopOperation;
    const operation = Promise.resolve().then(() => this.#performStop());
    this.#stopOperation = operation;
    try {
      await operation;
    } finally {
      if (this.#stopOperation === operation) this.#stopOperation = null;
    }
  }

  async configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]) {
    const configured = await this.request('activation.configure', {
      enabled,
      bindings: [...bindings],
    });
    this.#desiredActivation = Object.freeze({
      enabled: configured.enabled,
      bindings: Object.freeze(configured.bindings.map((binding) => Object.freeze(binding))),
    });
    return configured;
  }

  setSessionCapture(active: boolean) {
    return this.request('session.set_capture', { active });
  }

  async resetSessionCapture(): Promise<void> {
    await this.stop();
    if (this.#child !== null) {
      throw new HelperClientError(
        'transport-error',
        'Native helper capture reset could not confirm process exit',
      );
    }
    await this.start();
    if (this.#readiness.status !== 'ready') {
      throw new HelperClientError('not-running', 'Native helper capture reset is unavailable');
    }
    await this.setSessionCapture(false);
  }

  injectPaste(signal?: AbortSignal, onCommitted?: () => void): Promise<HelperPasteResult> {
    return this.request('paste.inject', {}, REQUEST_TIMEOUT_MS, signal, onCommitted);
  }

  getFrontApp(): Promise<HelperFrontApp> {
    return this.request('front_app.get', {});
  }

  async getPermissions(): Promise<HelperPermissions> {
    const [permissions, health] = await Promise.all([
      this.request('permissions.get', {}),
      this.request('ping', {}),
    ]);
    this.#setReadiness(
      readinessFromHandshake(this.#readiness.helperVersion, health.hookStatus, permissions),
    );
    return permissions;
  }

  ping() {
    return this.request('ping', {});
  }

  request<Method extends HelperMethod>(
    method: Method,
    params: HelperParams<Method>,
    timeoutMs = REQUEST_TIMEOUT_MS,
    signal?: AbortSignal,
    onPasteCommitted?: () => void,
    timeoutReason: HelperReadinessReason = 'request-timeout',
    allowStopping = false,
  ): Promise<HelperResult<Method>> {
    if (signal?.aborted === true) {
      return Promise.reject(new DOMException('Native helper request cancelled', 'AbortError'));
    }
    const child = this.#child;
    if (
      child === null ||
      this.#writeClosed ||
      (!this.#desiredRunning && !allowStopping) ||
      child.stdin.destroyed ||
      !child.stdin.writable
    ) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is terminating'));
    }

    if (this.#writeQueue.length >= OUTBOUND_QUEUE_CAPACITY) {
      return Promise.reject(
        new HelperClientError('transport-error', 'Native helper outbound queue is full'),
      );
    }

    const id = this.#takeRequestId();
    const validParams = helperParamsSchemas[method].parse(params);
    const frame = encodeHelperFrame({ jsonrpc: '2.0', id, method, params: validParams });
    return new Promise<HelperResult<Method>>((resolve, reject) => {
      const abort = (): void => {
        const pending = this.#pending.get(id);
        if (pending === undefined) return;
        if (pending.dispatched) {
          pending.abortRequested = true;
          return;
        }
        const queuedIndex = this.#writeQueue.findIndex((queued) => queued.id === id);
        if (queuedIndex !== -1) this.#writeQueue.splice(queuedIndex, 1);
        if (pending.timer !== null) clearTimeout(pending.timer);
        pending.removeAbort();
        this.#pending.delete(id);
        pending.reject(new DOMException('Native helper request cancelled', 'AbortError'));
      };
      signal?.addEventListener('abort', abort, { once: true });
      this.#pending.set(id, {
        method,
        deadlineAt: performance.now() + timeoutMs,
        timeoutReason,
        timer: null,
        dispatched: false,
        resolve: (result) => resolve(result as HelperResult<Method>),
        reject,
        removeAbort: () => signal?.removeEventListener('abort', abort),
        onPasteCommitted,
        abortRequested: false,
        pasteCommitted: false,
      });
      this.#armRequestTimeout(id);
      this.#writeQueue.push({ id, frame, child });
      this.#pumpWrites(child);
    });
  }

  #armRequestTimeout(id: number): void {
    const pending = this.#pending.get(id);
    if (pending === undefined) return;
    if (pending.timer !== null) clearTimeout(pending.timer);
    const remaining = Math.max(0, pending.deadlineAt - performance.now());
    pending.timer = setTimeout(() => this.#expireRequest(id), remaining);
    pending.timer.unref();
  }

  #expireRequest(id: number): void {
    const current = this.#pending.get(id);
    if (current === undefined) return;
    const queuedIndex = this.#writeQueue.findIndex((queued) => queued.id === id);
    if (queuedIndex !== -1) this.#writeQueue.splice(queuedIndex, 1);
    if (current.timer !== null) clearTimeout(current.timer);
    current.removeAbort();
    this.#pending.delete(id);
    current.reject(
      current.abortRequested
        ? new DOMException('Native helper request cancelled', 'AbortError')
        : new HelperClientError('request-timeout', `Native helper ${current.method} timed out`),
    );
    this.#terminateCurrent(current.timeoutReason, true);
  }

  #pumpWrites(child: ChildProcessWithoutNullStreams): void {
    if (this.#writeClosed || this.#writeBlocked || this.#child !== child) return;
    while (this.#writeQueue.length > 0) {
      const queued = this.#writeQueue.shift();
      if (queued === undefined) return;
      const pending = this.#pending.get(queued.id);
      if (pending === undefined) continue;
      if (queued.child !== child || child.stdin.destroyed || !child.stdin.writable) {
        this.#failTransport(child, 'Native helper stdin is unavailable');
        return;
      }
      if (performance.now() >= pending.deadlineAt) {
        this.#expireRequest(queued.id);
        return;
      }

      pending.dispatched = true;

      const writable = child.stdin.write(queued.frame, (error) => {
        if (error !== null && error !== undefined) {
          this.#failTransport(child, 'Native helper stdin failed');
        }
      });
      if (!writable) {
        this.#writeBlocked = true;
        return;
      }
    }
  }

  #failTransport(child: ChildProcessWithoutNullStreams, message: string): void {
    if (this.#child !== child) return;
    this.#terminateCurrent(
      'unexpected-exit',
      true,
      new HelperClientError('transport-error', message),
    );
  }

  async #performStop(): Promise<void> {
    this.#clearRestart();
    this.#clearHeartbeat();
    const child = this.#child;
    if (child !== null) {
      this.#plannedExit = { reason: 'shutdown', restart: false };
      await this.request(
        'session.set_capture',
        { active: false },
        250,
        undefined,
        undefined,
        'request-timeout',
        true,
      ).catch(() => undefined);
      await this.request(
        'activation.configure',
        { enabled: false, bindings: [] },
        250,
        undefined,
        undefined,
        'request-timeout',
        true,
      ).catch(() => undefined);
      const close = waitForClose(child);
      await this.request('shutdown', {}, 250, undefined, undefined, 'request-timeout', true).catch(
        () => undefined,
      );
      const closed = await Promise.race([
        close.then(() => true),
        delay(SHUTDOWN_TIMEOUT_MS).then(() => false),
      ]);
      if (!closed && this.#child === child) this.#terminateCurrent('shutdown', false);
      const forcedClosed =
        closed ||
        (await Promise.race([
          close.then(() => true),
          delay(SHUTDOWN_TIMEOUT_MS).then(() => false),
        ]));
      if (!forcedClosed && this.#child === child) {
        this.#rejectPending(
          new HelperClientError('transport-error', 'Native helper exit could not be confirmed'),
        );
        this.#setReadiness({
          status: 'unavailable',
          reason: 'hook-fault',
          helperVersion: this.#readiness.helperVersion,
          permissions: this.#readiness.permissions,
        });
        throw new HelperClientError('transport-error', 'Native helper exit could not be confirmed');
      }
    }
    if (!this.#desiredRunning) {
      this.#setReadiness({
        status: 'stopped',
        reason: 'shutdown',
        helperVersion: this.#readiness.helperVersion,
        permissions: this.#readiness.permissions,
      });
    }
  }

  async #launch(): Promise<void> {
    this.#clearHeartbeat();
    this.#setReadiness({
      status: 'starting',
      reason: null,
      helperVersion: null,
      permissions: DEFAULT_HELPER_PERMISSIONS,
    });
    try {
      await validateHelperExecutable(this.#options.executablePath, this.#options.platform);
    } catch (error) {
      const reason = error instanceof HelperBinaryError ? error.reason : 'binary-invalid';
      this.#setReadiness({
        status: 'unavailable',
        reason,
        helperVersion: null,
        permissions: DEFAULT_HELPER_PERMISSIONS,
      });
      return;
    }
    if (!this.#desiredRunning) return;

    this.#decoder = new HelperFrameDecoder();
    this.#writeQueue.length = 0;
    this.#writeBlocked = false;
    this.#writeClosed = true;
    this.#plannedExit = null;
    const generation = ++this.#generation;
    let child: ChildProcessWithoutNullStreams;
    try {
      child = this.#spawnHelper(this.#options.executablePath, {
        cwd: dirname(this.#options.executablePath),
        env: Object.freeze({ NO_COLOR: '1' }),
        shell: false,
        windowsHide: true,
      });
    } catch {
      this.#recordFailure('spawn-failed');
      return;
    }
    this.#child = child;
    this.#writeClosed = false;
    this.#attachProcess(child, generation);

    const launchDeadline = performance.now() + HANDSHAKE_TIMEOUT_MS;
    const remainingLaunchTime = () => launchDeadline - performance.now();
    try {
      const initialized = await this.request(
        'initialize',
        { protocolVersion: HELPER_PROTOCOL_VERSION },
        remainingLaunchTime(),
        undefined,
        undefined,
        'handshake-timeout',
      );
      if (!this.#isActiveChild(child)) return;
      this.#validateHandshake(initialized);
      if (this.#desiredActivation.enabled || this.#desiredActivation.bindings.length > 0) {
        await this.request(
          'activation.configure',
          {
            enabled: this.#desiredActivation.enabled,
            bindings: [...this.#desiredActivation.bindings],
          },
          remainingLaunchTime(),
          undefined,
          undefined,
          'handshake-timeout',
        );
      }
      if (!this.#isActiveChild(child)) return;
      this.#setReadiness(
        readinessFromHandshake(
          initialized.helperVersion,
          initialized.hookStatus,
          initialized.permissions,
        ),
      );
      this.#startHeartbeat(child);
    } catch (error) {
      if (this.#child !== child) return;
      const reason = classifyLaunchError(error);
      const stable = reason === 'protocol-mismatch';
      this.#terminateCurrent(reason, !stable);
    }
  }

  #validateHandshake(initialized: HelperInitializeResult): void {
    const expectedPlatform = this.#options.platform === 'win32' ? 'windows' : 'macos';
    const expectedArchitecture = this.#options.architecture === 'x64' ? 'x86_64' : 'aarch64';
    if (
      initialized.helperVersion !== this.#options.expectedHelperVersion ||
      initialized.platform !== expectedPlatform ||
      initialized.architecture !== expectedArchitecture
    ) {
      throw new HelperClientError('rpc-error', 'Native helper protocol or build is incompatible');
    }
  }

  #canLaunch(): boolean {
    return this.#desiredRunning && this.#child === null;
  }

  #wantsRunning(): boolean {
    return this.#desiredRunning;
  }

  #isActiveChild(child: ChildProcessWithoutNullStreams): boolean {
    return this.#wantsRunning() && this.#child === child;
  }

  #attachProcess(child: ChildProcessWithoutNullStreams, generation: number): void {
    child.stdin.on('drain', () => {
      if (this.#child !== child || this.#generation !== generation || this.#writeClosed) return;
      this.#writeBlocked = false;
      this.#pumpWrites(child);
    });
    child.stdin.once('error', () => this.#failTransport(child, 'Native helper stdin failed'));
    child.stdin.once('close', () => this.#failTransport(child, 'Native helper stdin closed'));
    child.stdout.once('error', () => this.#failTransport(child, 'Native helper stdout failed'));
    // stderr is diagnostic-only, but it still needs an error observer so a broken pipe cannot
    // become an uncaught main-process exception.
    child.stderr.once('error', () => undefined);
    child.stdout.on('data', (chunk: Buffer) => {
      if (this.#child !== child || this.#generation !== generation) return;
      try {
        for (const payload of this.#decoder.push(chunk)) this.#acceptPayload(payload);
      } catch {
        this.#terminateCurrent('malformed-response', true);
      }
    });
    child.stdout.once('end', () => {
      if (this.#child !== child) return;
      try {
        this.#decoder.finish();
      } catch {
        this.#terminateCurrent('malformed-response', true);
        return;
      }
      this.#terminateCurrent('unexpected-exit', true);
    });
    child.stderr.on('data', (chunk: Buffer) => this.#appendDiagnostics(chunk));
    child.once('error', () => {
      if (this.#child === child) this.#terminateCurrent('spawn-failed', true);
    });
    child.once('close', () => this.#handleClose(child, generation));
  }

  #acceptPayload(payload: Buffer): void {
    const raw = decodeHelperJson(payload);
    const response = HelperRpcResponseSchema.safeParse(raw);
    if (response.success) {
      if (response.data.id === null) throw new Error('Uncorrelated helper response');
      const pending = this.#pending.get(response.data.id);
      if (pending === undefined) throw new Error('Unknown helper response ID');
      if (pending.timer !== null) clearTimeout(pending.timer);
      pending.removeAbort();
      this.#pending.delete(response.data.id);
      if ('error' in response.data) {
        pending.reject(
          new HelperClientError(
            'rpc-error',
            `Native helper rejected ${pending.method}`,
            response.data.error.code,
          ),
        );
        return;
      }
      const result = helperResultSchemas[pending.method].safeParse(response.data.result);
      if (!result.success) {
        pending.reject(new HelperClientError('transport-error', 'Invalid helper result schema'));
        throw new Error('Invalid helper result schema');
      }
      pending.resolve(result.data);
      return;
    }

    const notification = HelperNotificationSchema.parse(raw);
    if (notification.method === 'paste.committed') {
      const pending = this.#pending.get(notification.params.requestId);
      if (pending?.method !== 'paste.inject') throw new Error('Unknown paste commit request ID');
      if (pending.pasteCommitted) return;
      pending.pasteCommitted = true;
      try {
        pending.onPasteCommitted?.();
      } catch {
        // Commit observers are application callbacks, not part of protocol supervision.
      }
      return;
    }
    for (const listener of this.#notificationListeners) {
      try {
        listener(notification);
      } catch {
        // Native protocol health must not depend on a consumer callback.
      }
    }
  }

  #handleClose(child: ChildProcessWithoutNullStreams, generation: number): void {
    if (this.#child !== child || this.#generation !== generation) return;
    this.#child = null;
    this.#writeQueue.length = 0;
    this.#writeBlocked = false;
    this.#writeClosed = true;
    this.#clearHeartbeat();
    this.#rejectPending(new HelperClientError('transport-error', 'Native helper stopped'));
    const planned = this.#plannedExit;
    this.#plannedExit = null;
    if (!this.#desiredRunning || this.#stopOperation !== null) return;
    this.#recordFailure(planned?.reason ?? 'unexpected-exit', planned?.restart ?? true);
  }

  #terminateCurrent(
    reason: HelperReadinessReason,
    restart: boolean,
    pendingError: Error = new HelperClientError(
      'transport-error',
      'Native helper transport is terminating',
    ),
  ): void {
    const child = this.#child;
    if (child === null || this.#writeClosed) return;
    this.#plannedExit ??= { reason, restart };
    // Close the write side synchronously. kill() and the eventual close event
    // are asynchronous, so neither may guard request admission or drain pumps.
    this.#writeClosed = true;
    this.#writeBlocked = false;
    this.#writeQueue.length = 0;
    this.#clearHeartbeat();
    this.#rejectPending(pendingError);
    child.kill();
  }

  #recordFailure(reason: HelperReadinessReason, restart = true): void {
    const now = Date.now();
    this.#failureTimes = this.#failureTimes.filter((time) => now - time < FAILURE_WINDOW_MS);
    this.#failureTimes.push(now);
    if (!restart || this.#failureTimes.length >= FAILURE_LIMIT) {
      this.#setReadiness({
        status: reason === 'protocol-mismatch' ? 'incompatible' : 'unavailable',
        reason: this.#failureTimes.length >= FAILURE_LIMIT ? 'crash-loop' : reason,
        helperVersion: this.#readiness.helperVersion,
        permissions: this.#readiness.permissions,
      });
      return;
    }

    this.#setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    const delayIndex = Math.min(this.#failureTimes.length - 1, RESTART_DELAYS_MS.length - 1);
    const restartAfter = RESTART_DELAYS_MS[delayIndex];
    this.#clearRestart();
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      if (this.#desiredRunning && this.#child === null) void this.start();
    }, restartAfter);
    this.#restartTimer.unref();
  }

  #startHeartbeat(child: ChildProcessWithoutNullStreams): void {
    this.#clearHeartbeat();
    this.#heartbeatTimer = setInterval(() => {
      if (this.#child !== child) return;
      void this.ping().catch(() => this.#terminateCurrent('request-timeout', true));
    }, HEARTBEAT_INTERVAL_MS);
    this.#heartbeatTimer.unref();
  }

  #setReadiness(readiness: HelperReadiness): void {
    const validated = HelperReadinessSchema.parse(readiness);
    if (JSON.stringify(validated) === JSON.stringify(this.#readiness)) return;
    this.#readiness = Object.freeze(validated);
    for (const listener of this.#readinessListeners) {
      try {
        listener(this.#readiness);
      } catch {
        // Readiness observers are isolated from helper supervision.
      }
    }
  }

  #takeRequestId(): number {
    const id = this.#nextRequestId;
    this.#nextRequestId = id === Number.MAX_SAFE_INTEGER ? 1 : id + 1;
    if (this.#pending.has(id))
      throw new HelperClientError('transport-error', 'Request ID exhausted');
    return id;
  }

  #rejectPending(error: Error): void {
    for (const pending of this.#pending.values()) {
      if (pending.timer !== null) clearTimeout(pending.timer);
      pending.removeAbort();
      pending.reject(error);
    }
    this.#pending.clear();
  }

  #appendDiagnostics(chunk: Buffer): void {
    const safe = chunk
      .toString('utf8')
      .replaceAll(/[^\t\n\r\x20-\x7e]/g, '\uFFFD')
      .slice(-STDERR_LIMIT_BYTES);
    this.#diagnostics = `${this.#diagnostics}${safe}`.slice(-STDERR_LIMIT_BYTES);
  }

  #clearRestart(): void {
    if (this.#restartTimer !== null) clearTimeout(this.#restartTimer);
    this.#restartTimer = null;
  }

  #clearHeartbeat(): void {
    if (this.#heartbeatTimer !== null) clearInterval(this.#heartbeatTimer);
    this.#heartbeatTimer = null;
  }
}

function defaultSpawnHelper(
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
): ChildProcessWithoutNullStreams {
  return spawn(executablePath, [], { ...options, stdio: ['pipe', 'pipe', 'pipe'] });
}

function readinessFromHandshake(
  helperVersion: string | null,
  hookStatus: HelperInitializeResult['hookStatus'],
  permissions: HelperPermissions,
): HelperReadiness {
  if (permissions.inputMonitoring === 'denied') {
    return {
      status: 'permission-required',
      reason: 'input-monitoring-required',
      helperVersion,
      permissions,
    };
  }
  if (permissions.accessibility === 'denied') {
    return {
      status: 'permission-required',
      reason: 'accessibility-required',
      helperVersion,
      permissions,
    };
  }
  if (permissions.eventPost === 'denied') {
    return {
      status: 'permission-required',
      reason: 'event-post-required',
      helperVersion,
      permissions,
    };
  }
  if (hookStatus !== 'ready') {
    return { status: 'unavailable', reason: 'hook-fault', helperVersion, permissions };
  }
  return { status: 'ready', reason: null, helperVersion, permissions };
}

function classifyLaunchError(error: unknown): HelperReadinessReason {
  if (error instanceof HelperClientError) {
    if (error.code === 'request-timeout') return 'handshake-timeout';
    if (
      error.code === 'rpc-error' &&
      (error.rpcCode === -32_001 || error.message.includes('incompatible'))
    ) {
      return 'protocol-mismatch';
    }
    if (error.code === 'rpc-error') return 'hook-fault';
  }
  return 'malformed-response';
}

function waitForClose(child: ChildProcessWithoutNullStreams): Promise<void> {
  if (child.exitCode !== null || child.signalCode !== null) return Promise.resolve();
  return new Promise((resolve) => child.once('close', () => resolve()));
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}
