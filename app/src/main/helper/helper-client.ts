import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { dirname } from 'node:path';
import { performance } from 'node:perf_hooks';
import {
  HELPER_PROTOCOL_VERSION,
  type ActivationBinding,
  type HelperFrontApp,
  type HelperInitializeResult,
  type HelperMethod,
  type HelperNotification,
  type HelperParams,
  type HelperPasteResult,
  type HelperPermissions,
  type HelperResult,
  type HelperSessionCaptureMode,
} from '../../shared/helper/protocol';
import {
  DEFAULT_HELPER_PERMISSIONS,
  HelperReadinessSchema,
  INITIAL_HELPER_READINESS,
  type HelperReadiness,
  type HelperReadinessReason,
} from '../../shared/schemas/helper-readiness';
import { HelperActivationReconciler } from './helper-activation-reconciler';
import { HelperRpcChannel, type HelperRpcSession } from './helper-rpc-channel';
import { HelperBinaryError, type HelperPlatform, validateHelperExecutable } from './helper-path';

const HANDSHAKE_TIMEOUT_MS = 3_000;
const REQUEST_TIMEOUT_MS = 3_000;
const HEARTBEAT_INTERVAL_MS = 5_000;
const SHUTDOWN_TIMEOUT_MS = 1_000;
const FAILURE_WINDOW_MS = 2 * 60_000;
const FAILURE_LIMIT = 5;
const RESTART_DELAYS_MS = [250, 1_000, 4_000, 15_000, 30_000] as const;

type SpawnHelper = (
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

export interface HelperClientOptions {
  readonly executablePath: string;
  readonly expectedHelperVersion: string;
  readonly platform: HelperPlatform;
  readonly architecture: 'x64' | 'arm64';
  readonly spawnHelper?: SpawnHelper;
}

export class HelperClientError extends Error {
  readonly code:
    'not-running' | 'request-capacity' | 'request-timeout' | 'rpc-error' | 'transport-error';
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
  readonly #rpcChannel: HelperRpcChannel;
  readonly #activation: HelperActivationReconciler;
  readonly #readinessListeners = new Set<(readiness: HelperReadiness) => void>();
  readonly #notificationListeners = new Set<(notification: HelperNotification) => void>();
  #child: ChildProcessWithoutNullStreams | null = null;
  #rpcSession: HelperRpcSession | null = null;
  #terminating = false;
  #generation = 0;
  #runIntentRevision = 0;
  #desiredRunning = false;
  #healthRefresh: {
    readonly session: HelperRpcSession | null;
    readonly operation: Promise<HelperPermissions>;
  } | null = null;
  #launching: Promise<void> | null = null;
  #stopOperation: Promise<void> | null = null;
  #restartTimer: NodeJS.Timeout | null = null;
  #heartbeatTimer: NodeJS.Timeout | null = null;
  #plannedExit: { readonly reason: HelperReadinessReason; readonly restart: boolean } | null = null;
  #failureTimes: number[] = [];
  #readiness: HelperReadiness = INITIAL_HELPER_READINESS;

  constructor(options: HelperClientOptions) {
    this.#options = options;
    this.#spawnHelper = options.spawnHelper ?? defaultSpawnHelper;
    this.#rpcChannel = new HelperRpcChannel({
      createError: (code, message, rpcCode = null) => new HelperClientError(code, message, rpcCode),
      onFault: (session, reason, pendingError) =>
        this.#handleRpcFault(session, reason, pendingError),
      onNotification: (session, notification) => this.#publishNotification(session, notification),
    });
    this.#activation = new HelperActivationReconciler({
      getSession: () => this.#rpcSession,
      isSessionAvailable: (session) =>
        this.#desiredRunning && this.#stopOperation === null && this.#rpcChannel.isCurrent(session),
      request: (session, params, timeoutMs, timeoutReason) =>
        this.#rpcChannel.request(session, 'activation.configure', params, {
          timeoutMs,
          timeoutReason,
          allowDraining: false,
          supervision: true,
        }),
      createNotRunningError: (message) => new HelperClientError('not-running', message),
      isNotRunningError: (error) =>
        error instanceof HelperClientError && error.code === 'not-running',
    });
  }

  get readiness(): HelperReadiness {
    return this.#readiness;
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
    const revision = ++this.#runIntentRevision;
    this.#desiredRunning = true;
    await this.#startForIntent(revision);
  }

  async restart(): Promise<void> {
    const revision = ++this.#runIntentRevision;
    this.#desiredRunning = true;
    const stopping = this.#stopOperation;
    if (stopping !== null) await stopping;
    if (!this.#intentIsCurrent(revision)) return;
    this.#clearRestart();
    const child = this.#child;
    if (child !== null) {
      const close = waitForClose(child);
      try {
        this.#terminateCurrent('unexpected-exit', true);
        const closed = await waitForCloseWithin(close.promise, SHUTDOWN_TIMEOUT_MS);
        if (!closed) {
          throw new HelperClientError(
            'transport-error',
            'Native helper restart could not confirm process exit',
          );
        }
        if (!this.#intentIsCurrent(revision)) return;
        this.#clearRestart();
      } finally {
        close.cancel();
      }
    }
    await this.#startForIntent(revision);
  }

  async stop(): Promise<void> {
    this.#runIntentRevision += 1;
    this.#desiredRunning = false;
    await this.#stopCurrentProcess();
  }

  configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]) {
    return this.#activation.configure(enabled, bindings);
  }

  setSessionCapture(mode: HelperSessionCaptureMode) {
    return this.request('session.set_capture', { mode });
  }

  async resetSessionCapture(signal?: AbortSignal): Promise<void> {
    const revision = this.#runIntentRevision;
    this.#assertResetAllowed(revision, signal);
    const stopOnAbort = (): void => {
      void this.stop().catch(() => undefined);
    };
    signal?.addEventListener('abort', stopOnAbort, { once: true });
    try {
      await this.#stopCurrentProcess();
      this.#assertResetAllowed(revision, signal);
      if (this.#child !== null) {
        throw new HelperClientError(
          'transport-error',
          'Native helper capture reset could not confirm process exit',
        );
      }
      await this.#startForIntent(revision);
      this.#assertResetAllowed(revision, signal);
      if (this.#readiness.status !== 'ready') {
        throw new HelperClientError('not-running', 'Native helper capture reset is unavailable');
      }
      await this.setSessionCapture('off');
      this.#assertResetAllowed(revision, signal);
    } catch (error: unknown) {
      if (signal?.aborted === true || !this.#desiredRunning) {
        await this.#stopCurrentProcess().catch(() => undefined);
      }
      throw error;
    } finally {
      signal?.removeEventListener('abort', stopOnAbort);
    }
  }

  injectPaste(signal?: AbortSignal, onCommitted?: () => void): Promise<HelperPasteResult> {
    return this.request('paste.inject', {}, REQUEST_TIMEOUT_MS, signal, onCommitted);
  }

  getFrontApp(): Promise<HelperFrontApp> {
    return this.request('front_app.get', {});
  }

  getPermissions(): Promise<HelperPermissions> {
    const session = this.#rpcSession;
    if (this.#healthRefresh?.session === session) return this.#healthRefresh.operation;
    const operation = this.#refreshHealth(session).finally(() => {
      if (this.#healthRefresh?.operation === operation) this.#healthRefresh = null;
    });
    this.#healthRefresh = { session, operation };
    return operation;
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
    supervision = false,
  ): Promise<HelperResult<Method>> {
    if (signal?.aborted === true) {
      return Promise.reject(new DOMException('Native helper request cancelled', 'AbortError'));
    }
    const session = this.#rpcSession;
    if (session === null || (!this.#desiredRunning && !allowStopping)) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is terminating'));
    }
    return this.#rpcChannel.request(session, method, params, {
      timeoutMs,
      timeoutReason,
      signal,
      onPasteCommitted,
      allowDraining: allowStopping,
      supervision,
    });
  }

  async #refreshHealth(session: HelperRpcSession | null): Promise<HelperPermissions> {
    if (session === null || !this.#desiredRunning) {
      throw new HelperClientError('not-running', 'Native helper is terminating');
    }
    const [permissions, health] = await Promise.all([
      this.#rpcChannel.request(
        session,
        'permissions.get',
        {},
        {
          timeoutMs: REQUEST_TIMEOUT_MS,
          timeoutReason: 'request-timeout',
          allowDraining: false,
          supervision: true,
        },
      ),
      this.#rpcChannel.request(
        session,
        'ping',
        {},
        {
          timeoutMs: REQUEST_TIMEOUT_MS,
          timeoutReason: 'request-timeout',
          allowDraining: false,
          supervision: true,
        },
      ),
    ]);
    if (!this.#healthSessionIsActive(session)) return permissions;
    const readiness = readinessFromHandshake(
      this.#readiness.helperVersion,
      health.hookStatus,
      permissions,
    );
    this.#activation.setBlockedByHealth(readiness.status !== 'ready');
    try {
      await this.#activation.reconcileSession(session, false);
    } catch (error: unknown) {
      if (!this.#healthSessionIsActive(session)) return permissions;
      this.#terminateCurrent(readiness.reason ?? 'hook-fault', true);
      throw error;
    }
    if (!this.#healthSessionIsActive(session)) return permissions;
    this.#setReadiness(readiness);
    if (
      readiness.status === 'unavailable' &&
      permissionsAreGranted(permissions) &&
      health.hookStatus !== 'ready'
    ) {
      // A macOS event tap created without permission cannot become live in place.
      // Recycle only after activation is confirmed disabled so the replacement
      // can recreate the hook and restore the retained desired configuration.
      this.#terminateCurrent('hook-fault', true);
    }
    return permissions;
  }

  #healthSessionIsActive(session: HelperRpcSession): boolean {
    return (
      this.#rpcSession === session &&
      this.#desiredRunning &&
      this.#stopOperation === null &&
      this.#rpcChannel.isCurrent(session)
    );
  }

  async #startForIntent(revision: number): Promise<void> {
    const stopping = this.#stopOperation;
    if (stopping !== null) await stopping;
    if (!this.#intentIsCurrent(revision)) return;
    if (this.#launching !== null) {
      await this.#launching;
      return;
    }
    if (!this.#canLaunch()) return;
    this.#launching = this.#launch().finally(() => {
      this.#launching = null;
    });
    await this.#launching;
  }

  async #stopCurrentProcess(): Promise<void> {
    const session = this.#rpcSession;
    if (session !== null) this.#rpcChannel.beginDraining(session);
    if (this.#stopOperation !== null) return this.#stopOperation;
    const operation = Promise.resolve().then(() => this.#performStop());
    this.#stopOperation = operation;
    try {
      await operation;
    } finally {
      if (this.#stopOperation === operation) this.#stopOperation = null;
    }
  }

  #intentIsCurrent(revision: number): boolean {
    return this.#desiredRunning && this.#runIntentRevision === revision;
  }

  #assertResetAllowed(revision: number, signal?: AbortSignal): void {
    if (signal?.aborted === true) {
      throw new DOMException('Native helper capture reset cancelled', 'AbortError');
    }
    if (!this.#intentIsCurrent(revision)) {
      throw new HelperClientError('not-running', 'Native helper capture reset was superseded');
    }
  }

  async #performStop(): Promise<void> {
    this.#clearRestart();
    this.#clearHeartbeat();
    const child = this.#child;
    if (child !== null) {
      this.#plannedExit = { reason: 'shutdown', restart: false };
      const close = waitForClose(child);
      try {
        await this.request(
          'session.set_capture',
          { mode: 'off' },
          250,
          undefined,
          undefined,
          'request-timeout',
          true,
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
          true,
        ).catch(() => undefined);
        await this.request(
          'shutdown',
          {},
          250,
          undefined,
          undefined,
          'request-timeout',
          true,
          true,
        ).catch(() => undefined);
        const closed = await waitForCloseWithin(close.promise, SHUTDOWN_TIMEOUT_MS);
        if (!closed && this.#child === child) this.#terminateCurrent('shutdown', false);
        const forcedClosed =
          closed || (await waitForCloseWithin(close.promise, SHUTDOWN_TIMEOUT_MS));
        if (!forcedClosed && this.#child === child) {
          const error = new HelperClientError(
            'transport-error',
            'Native helper exit could not be confirmed',
          );
          const session = this.#rpcSession;
          if (session !== null) this.#rpcChannel.close(session, error);
          this.#setReadiness({
            status: 'unavailable',
            reason: 'hook-fault',
            helperVersion: this.#readiness.helperVersion,
            permissions: this.#readiness.permissions,
          });
          throw error;
        }
      } finally {
        close.cancel();
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
      if (!this.#desiredRunning) return;
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

    this.#plannedExit = null;
    this.#terminating = false;
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
    const session = this.#rpcChannel.attach(child);
    this.#rpcSession = session;
    this.#activation.prepareFreshSession();
    this.#attachProcess(child, session, generation);

    const launchDeadline = performance.now() + HANDSHAKE_TIMEOUT_MS;
    const remainingLaunchTime = () => launchDeadline - performance.now();
    try {
      const initialized = await this.#rpcChannel.request(
        session,
        'initialize',
        { protocolVersion: HELPER_PROTOCOL_VERSION },
        {
          timeoutMs: remainingLaunchTime(),
          timeoutReason: 'handshake-timeout',
          allowDraining: false,
          supervision: false,
        },
      );
      if (!this.#isActiveChild(child, session)) return;
      this.#validateHandshake(initialized);
      let readiness = readinessFromHandshake(
        initialized.helperVersion,
        initialized.hookStatus,
        initialized.permissions,
      );
      // A fresh helper is natively disabled. Keep it blocked until a second
      // health snapshot confirms that replaying retained activation is safe.
      this.#activation.setBlockedByHealth(true);
      if (readiness.status === 'ready') {
        const [permissions, health] = await Promise.all([
          this.#rpcChannel.request(
            session,
            'permissions.get',
            {},
            {
              timeoutMs: remainingLaunchTime(),
              timeoutReason: 'handshake-timeout',
              allowDraining: false,
              supervision: true,
            },
          ),
          this.#rpcChannel.request(
            session,
            'ping',
            {},
            {
              timeoutMs: remainingLaunchTime(),
              timeoutReason: 'handshake-timeout',
              allowDraining: false,
              supervision: true,
            },
          ),
        ]);
        readiness = readinessFromHandshake(
          initialized.helperVersion,
          health.hookStatus,
          permissions,
        );
      }
      if (readiness.status === 'ready') this.#activation.setBlockedByHealth(false);
      await this.#activation.reconcileFreshHelper(
        session,
        remainingLaunchTime(),
        'handshake-timeout',
      );
      if (!this.#isActiveChild(child, session)) return;
      this.#setReadiness(readiness);
      this.#startHeartbeat(child, session);
    } catch (error) {
      if (this.#child !== child || this.#rpcSession !== session) return;
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

  #isActiveChild(child: ChildProcessWithoutNullStreams, session: HelperRpcSession): boolean {
    return this.#desiredRunning && this.#child === child && this.#rpcSession === session;
  }

  #attachProcess(
    child: ChildProcessWithoutNullStreams,
    session: HelperRpcSession,
    generation: number,
  ): void {
    // stderr is diagnostic-only, but it still needs an error observer so a broken pipe cannot
    // become an uncaught main-process exception.
    child.stderr.once('error', () => undefined);
    child.stderr.on('data', () => undefined);
    child.once('error', () => {
      if (this.#child === child && this.#rpcSession === session) {
        this.#terminateCurrent('spawn-failed', true);
      }
    });
    child.once('close', () => this.#handleClose(child, session, generation));
  }

  #handleRpcFault(
    session: HelperRpcSession,
    reason: HelperReadinessReason,
    pendingError?: Error,
  ): void {
    if (this.#rpcSession !== session) return;
    this.#terminateCurrent(reason, true, pendingError);
  }

  #publishNotification(session: HelperRpcSession, notification: HelperNotification): void {
    if (this.#rpcSession !== session) return;
    for (const listener of this.#notificationListeners) {
      try {
        listener(notification);
      } catch {
        // Native protocol health must not depend on a consumer callback.
      }
    }
  }

  #handleClose(
    child: ChildProcessWithoutNullStreams,
    session: HelperRpcSession,
    generation: number,
  ): void {
    if (this.#child !== child || this.#rpcSession !== session || this.#generation !== generation) {
      return;
    }
    this.#rpcChannel.close(
      session,
      new HelperClientError('transport-error', 'Native helper stopped'),
    );
    this.#activation.processUnavailable(session);
    this.#child = null;
    this.#rpcSession = null;
    this.#terminating = false;
    this.#clearHeartbeat();
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
    const session = this.#rpcSession;
    if (child === null || session === null || this.#terminating) return;
    this.#terminating = true;
    this.#plannedExit ??= { reason, restart };
    // Close request admission synchronously. kill() and the eventual close event
    // are asynchronous, so neither may guard request admission or drain pumps.
    this.#activation.processUnavailable(session);
    this.#clearHeartbeat();
    this.#setReadiness({
      status: !this.#desiredRunning && reason === 'shutdown' ? 'stopped' : 'unavailable',
      reason,
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    this.#rpcChannel.close(session, pendingError);
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
    const revision = this.#runIntentRevision;
    this.#clearRestart();
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      if (this.#intentIsCurrent(revision) && this.#child === null) {
        void this.#startForIntent(revision);
      }
    }, restartAfter);
    this.#restartTimer.unref();
  }

  #startHeartbeat(child: ChildProcessWithoutNullStreams, session: HelperRpcSession): void {
    this.#clearHeartbeat();
    this.#heartbeatTimer = setInterval(() => {
      if (this.#child !== child || this.#rpcSession !== session) return;
      void this.getPermissions().catch((error: unknown) => {
        if (error instanceof HelperClientError && error.code === 'request-capacity') return;
        if (this.#rpcSession === session && this.#desiredRunning && this.#stopOperation === null) {
          this.#terminateCurrent('request-timeout', true);
        }
      });
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

function permissionsAreGranted(permissions: HelperPermissions): boolean {
  return Object.values(permissions).every(
    (permission) => permission === 'granted' || permission === 'not_applicable',
  );
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

interface CloseWaiter {
  readonly promise: Promise<void>;
  readonly cancel: () => void;
}

function waitForClose(child: ChildProcessWithoutNullStreams): CloseWaiter {
  let settled = false;
  let resolveClose: () => void = () => undefined;
  const onClose = (): void => {
    settled = true;
    resolveClose();
  };
  const promise = new Promise<void>((resolve) => {
    resolveClose = resolve;
    child.once('close', onClose);
  });
  return {
    promise,
    cancel: () => {
      if (!settled) child.removeListener('close', onClose);
    },
  };
}

async function waitForCloseWithin(close: Promise<void>, milliseconds: number): Promise<boolean> {
  let resolveTimeout: (value: boolean) => void = () => undefined;
  const timeout = new Promise<boolean>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(() => resolveTimeout(false), milliseconds);
  timer.unref();
  try {
    return await Promise.race([close.then(() => true), timeout]);
  } finally {
    clearTimeout(timer);
  }
}
