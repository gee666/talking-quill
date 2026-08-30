import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';
import { dirname, isAbsolute } from 'node:path';
import { performance } from 'node:perf_hooks';
import { StringDecoder } from 'node:string_decoder';
import { z } from 'zod';
import {
  HELPER_PROTOCOL_VERSION,
  HelperDiagnosticIdentitySchema,
  HelperRuntimeObservabilitySchema,
  HelperTerminalObservabilityRecordSchema,
  type ActivationBinding,
  type HelperAcceptanceEndpointObservability,
  type HelperAcceptancePauseLeaseRenewalResult,
  type HelperActivationContext,
  type HelperFrontApp,
  type HelperInitializeResult,
  type HelperKeyboardOwnerSnapshot,
  type HelperMethod,
  type HelperNotification,
  type HelperParams,
  type HelperPasteResult,
  type HelperPermissions,
  type HelperPrepareMaintenanceParams,
  type HelperResult,
  type HelperRuntimeObservability,
  type HelperSessionCaptureMode,
  type HelperTerminalObservabilityRecord,
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

export const ACTIVATION_CAPTURE_ROLLBACK_ENV = 'TALKING_QUILL_DISABLE_ACTIVATION_CAPTURE' as const;

export function activationCaptureRollbackEnabled(environment: NodeJS.ProcessEnv): boolean {
  return environment[ACTIVATION_CAPTURE_ROLLBACK_ENV] === '1';
}

// The gateway launches the adjacent owner and completes their mutually
// authenticated private handshake before it accepts Electron RPC work.
const HANDSHAKE_TIMEOUT_MS = 30_000;
const REQUEST_TIMEOUT_MS = 3_000;
const ACCEPTANCE_LEASE_RENEWAL_TIMEOUT_MS = 10_000;
const HEARTBEAT_INTERVAL_MS = 5_000;
// Both native adapters own one 1.5-second drain deadline. The host waits a
// comfortably larger platform envelope before best-effort termination.
const MAC_SHUTDOWN_WAIT_MS = 3_000;
const WINDOWS_SHUTDOWN_WAIT_MS = 3_000;
const PREDECESSOR_DRAIN_WAIT_MS = 3_000;
const SHUTDOWN_EXIT_MARGIN_MS = 500;
const MAX_TERMINAL_OBSERVABILITY_LINE_BYTES = 16 * 1024;
const MAX_DIAGNOSTIC_ACKS_IN_FLIGHT = 64;
const OWNER_DIAGNOSTIC_JOURNAL_ENV = 'TALKING_QUILL_OWNER_DIAGNOSTIC_JOURNAL_V1';
const STARTUP_STDERR_DRAIN_MS = 250;
const FAILURE_WINDOW_MS = 2 * 60_000;
const FAILURE_LIMIT = 5;
const RESTART_DELAYS_MS = [250, 1_000, 4_000, 15_000, 30_000] as const;

type SpawnHelper = (
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
) => ChildProcessWithoutNullStreams;

export type HelperRuntimeObservabilitySource = 'runtime' | 'shutdown' | 'failure';

const HelperOwnerConnectionDiagnosticSchema = z
  .object({
    event: z.literal('helper.owner.connection.replay'),
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
    streamId: HelperDiagnosticIdentitySchema,
    processGeneration: z.string().regex(/^[1-9][0-9]{0,19}$/u),
    category: z.enum([
      'connect',
      'disconnected',
      'uncertain',
      'protocol',
      'acquire_rejected',
      'rejected',
      'sequence_exhausted',
      'transport',
    ]),
    operation: z.enum([
      'capture.replace_configuration',
      'capture.set_enabled',
      'command.sequence',
      'connect.reconcile',
      'established_operation',
      'front_app.get',
      'front_app.metadata.get',
      'front_app.metadata_get',
      'health.get',
      'lease.acquire',
      'lease.release',
      'lease.renew',
      'observability.get',
      'paste.await_commit',
      'paste.inject',
      'permissions.get',
      'runtime.rollback',
      'service',
      'service.poll',
      'session.reconcile_off',
      'session.set_mode',
    ]),
    correlationStatus: z.enum([
      'none',
      'not_established',
      'pending',
      'matched',
      'matched_initial_response',
      'mismatched',
      'unexpected_response',
      'unknown',
    ]),
    healthRefresh: z.enum(['not_attempted', 'succeeded', 'failed']),
    transportStatus: z.enum(['open', 'eof', 'closed', 'error', 'backpressured', 'unknown']),
    ownerProcessState: z.enum(['running', 'exited', 'unknown']),
    count: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    counterOverflow: z.boolean(),
    durable: z.boolean(),
    durabilityFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    writerStartFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    synchronizationRecoveries: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
  })
  .strict();
export type HelperOwnerConnectionDiagnostic = z.infer<typeof HelperOwnerConnectionDiagnosticSchema>;

export interface HelperClientOptions {
  readonly executablePath: string;
  readonly expectedHelperVersion: string;
  readonly diagnosticJournalPath?: string;
  readonly platform: HelperPlatform;
  readonly architecture: 'x64' | 'arm64';
  /** Internal emergency gate; it disables global activation in the helper process. */
  readonly disableActivationCapture?: boolean;
  /** Test-only timing override; production uses the platform shutdown envelope. */
  readonly nativeDrainEnvelopeMs?: number;
  /** Test-only timing override for the one dispatched predecessor. */
  readonly predecessorDrainEnvelopeMs?: number;
  readonly observeRuntimeObservability?: (
    observability: HelperRuntimeObservability,
    source: HelperRuntimeObservabilitySource,
  ) => void | Promise<void>;
  readonly observeProcessLifecycle?: (event: {
    readonly phase: 'started' | 'exited';
    readonly exitCode?: number | null;
    readonly signal?: NodeJS.Signals | null;
    readonly planned?: boolean;
  }) => void | Promise<void>;
  readonly observeOwnerConnectionDiagnostic?: (
    diagnostic: HelperOwnerConnectionDiagnostic,
  ) => Promise<boolean>;
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
  readonly #shutdownWaitMs: number;
  readonly #predecessorDrainWaitMs: number;
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
  #stopTerminalFault: Error | null = null;
  #restartTimer: NodeJS.Timeout | null = null;
  #heartbeatTimer: NodeJS.Timeout | null = null;
  #plannedExit: {
    readonly reason: HelperReadinessReason;
    readonly restart: boolean;
    readonly lifecycle: boolean;
  } | null = null;
  #failureTimes: number[] = [];
  #crashLoopOpen = false;
  #halfOpenProbe = false;
  #sessionAuthoritative = false;
  #maintenancePreparing = false;
  #maintenancePrepared = false;
  #captureDisabledForSession = false;
  #captureBuildDisabledForSession = false;
  #runtimeRollbackForSession = false;
  #sessionKeyCaptureAvailable: boolean | null = null;
  #ownerAssociation: {
    readonly instanceId: string;
    readonly buildId: string;
    readonly leaseEpoch: number | null;
  } | null = null;
  #readiness: HelperReadiness = INITIAL_HELPER_READINESS;
  #nativeLaunchFailure: string | null = null;
  #stderrDrain: {
    readonly generation: number;
    readonly promise: Promise<void>;
    readonly complete: () => void;
    completed: boolean;
  } | null = null;
  readonly #diagnosticAcksInFlight = new Map<string, Promise<void>>();
  #electronRegisteredObservations = 0;
  #physicalObservationsAccepted = 0;
  #pendingAuthoritativeActivation: Extract<HelperNotification, { method: 'activation.event' }>[] =
    [];
  // Gestures observed behind an owner/configuration fence have an explicit disposition. Transient
  // authenticated reconciliation may replay one current gesture; terminal and unauthenticated
  // states always discard it.
  #pendingActivationPolicy: 'drop' | 'replay-current-gesture' = 'drop';

  constructor(options: HelperClientOptions) {
    if (
      options.diagnosticJournalPath !== undefined &&
      (!isAbsolute(options.diagnosticJournalPath) || options.diagnosticJournalPath.includes('\0'))
    ) {
      throw new Error('Helper diagnostic journal path must be absolute');
    }
    this.#options = options;
    this.#shutdownWaitMs =
      options.nativeDrainEnvelopeMs ??
      (options.platform === 'darwin' ? MAC_SHUTDOWN_WAIT_MS : WINDOWS_SHUTDOWN_WAIT_MS);
    this.#predecessorDrainWaitMs = options.predecessorDrainEnvelopeMs ?? PREDECESSOR_DRAIN_WAIT_MS;
    this.#spawnHelper = options.spawnHelper ?? defaultSpawnHelper;
    this.#rpcChannel = new HelperRpcChannel({
      createError: (code, message, rpcCode = null) => new HelperClientError(code, message, rpcCode),
      onFault: (session, reason, pendingError) =>
        this.#handleRpcFault(session, reason, pendingError),
      onNotification: (session, notification) => this.#publishNotification(session, notification),
      onPingResult: (session, result) => {
        if (
          this.#rpcSession === session &&
          this.#ownerAssociation !== null &&
          !this.#ownerAssociationMatches(result.keyboardOwner)
        ) {
          this.#pendingAuthoritativeActivation.length = 0;
          this.#pendingActivationPolicy = 'replay-current-gesture';
          this.#rpcChannel.resetOwnerActivationStream(session);
          this.#sessionAuthoritative = false;
          if (this.#healthRefresh?.session !== session && this.#launching === null) {
            this.#setReadiness({
              status: 'starting',
              reason: null,
              helperVersion: this.#readiness.helperVersion,
              permissions: this.#readiness.permissions,
            });
            queueMicrotask(() => {
              void this.#reconcileLocalOwnerChange(
                session,
                result.keyboardOwner,
                result.hookStatus,
                this.#readiness.permissions,
              ).catch(() => {
                if (this.#healthSessionIsActive(session)) {
                  this.#terminateCurrent('owner-degraded', true);
                }
              });
            });
          }
        }
      },
    });
    this.#activation = new HelperActivationReconciler({
      getSession: () => this.#rpcSession,
      isSessionAvailable: (session) =>
        this.#sessionAuthoritative &&
        !this.#maintenancePreparing &&
        !this.#maintenancePrepared &&
        !this.#terminating &&
        this.#desiredRunning &&
        this.#stopOperation === null &&
        this.#rpcChannel.isCurrent(session),
      isSessionCurrent: (session) =>
        !this.#maintenancePreparing &&
        !this.#maintenancePrepared &&
        !this.#terminating &&
        this.#desiredRunning &&
        this.#stopOperation === null &&
        this.#rpcChannel.isCurrent(session),
      request: (session, params, timeoutMs, timeoutReason) =>
        this.#rpcChannel.request(session, 'activation.configure', params, {
          timeoutMs,
          timeoutReason,
          allowDraining: false,
          supervision: true,
        }),
      createNotRunningError: (message) => new HelperClientError('not-running', message),
      createProtocolError: (message) => new HelperClientError('rpc-error', message),
      reportProtocolFault: (session, error) =>
        this.#handleRpcFault(session, 'malformed-response', error),
      isNotRunningError: (error) =>
        error instanceof HelperClientError && error.code === 'not-running',
    });
  }

  get readiness(): HelperReadiness {
    return this.#readiness;
  }

  /** Privacy-safe native launcher failure classification for installed diagnostics. */
  get nativeLaunchFailure(): string | null {
    return this.#nativeLaunchFailure;
  }

  /** Effective helper acknowledgement; null while no helper session is authoritative. */
  get activationCaptureEnabled(): boolean | null {
    return this.#activation.effectiveEnabled;
  }

  /** Owner-lease capability from the protocol-v10 keyboardCapture handshake. */
  get sessionKeyCaptureAvailable(): boolean | null {
    return this.#sessionKeyCaptureAvailable;
  }

  subscribeReadiness(listener: (readiness: HelperReadiness) => void): () => void {
    this.#readinessListeners.add(listener);
    return () => this.#readinessListeners.delete(listener);
  }

  subscribeNotifications(listener: (notification: HelperNotification) => void): () => void {
    this.#notificationListeners.add(listener);
    return () => this.#notificationListeners.delete(listener);
  }

  subscribeInputDeviceInvalidations(listener: () => void): () => void {
    return this.subscribeNotifications((notification) => {
      if (notification.method === 'audio.input_devices_changed') listener();
    });
  }

  async start(): Promise<void> {
    const revision = ++this.#runIntentRevision;
    this.#desiredRunning = true;
    this.#clearRestart();
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
        const closed = await waitForCloseWithin(
          close.promise,
          this.#shutdownWaitMs + SHUTDOWN_EXIT_MARGIN_MS,
        );
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

  async stop(options: { readonly requireNeutral?: boolean } = {}): Promise<void> {
    this.#runIntentRevision += 1;
    this.#desiredRunning = false;
    this.#failureTimes = [];
    this.#crashLoopOpen = false;
    this.#halfOpenProbe = false;
    await this.#stopCurrentProcess(options.requireNeutral === true);
  }

  configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]) {
    if (this.#maintenancePreparing || this.#maintenancePrepared) {
      return Promise.reject(
        new HelperClientError('not-running', 'Native helper is in maintenance'),
      );
    }
    return this.#activation.configure(enabled, bindings);
  }

  async beginPhysicalObservation(): Promise<HelperRuntimeObservability> {
    if (this.#options.platform !== 'win32') {
      throw new HelperClientError(
        'not-running',
        'Passive registered-input observation is unavailable on this platform',
      );
    }
    // First confirm the owner has applied disabled activation with retained
    // bindings. Only then take the baseline, so a chord pressed during the
    // transition is historical and cannot satisfy the armed observation.
    await this.#activation.beginPhysicalObservation();
    return this.getRuntimeObservability();
  }

  samplePhysicalObservation(): Promise<HelperRuntimeObservability> {
    return this.getRuntimeObservability();
  }

  async endPhysicalObservation(): Promise<void> {
    await this.#activation.endPhysicalObservation();
  }

  setSessionCapture(mode: HelperSessionCaptureMode) {
    if (!this.#ordinaryRequestsAvailable()) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is initializing'));
    }
    if (this.#sessionKeyCaptureAvailable === false) {
      return Promise.resolve({ mode: 'off' as const });
    }
    return this.request('session.set_capture', { mode });
  }

  async resetSessionCapture(signal?: AbortSignal): Promise<void> {
    if (this.#sessionKeyCaptureAvailable === false) return;
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

  injectPaste(
    activationContext: Readonly<HelperActivationContext>,
    expectedClipboardSha256: string,
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<HelperPasteResult> {
    return this.request(
      'paste.inject',
      { ...activationContext, expectedClipboardSha256 },
      REQUEST_TIMEOUT_MS,
      signal,
      onCommitted,
    );
  }

  getFrontApp(): Promise<HelperFrontApp> {
    return this.request('front_app.get', {});
  }

  getPermissions(): Promise<HelperPermissions> {
    if (!this.#ordinaryRequestsAvailable()) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is initializing'));
    }
    return this.#refreshHealthCoalesced(this.#rpcSession);
  }

  #refreshHealthCoalesced(session: HelperRpcSession | null): Promise<HelperPermissions> {
    if (this.#healthRefresh?.session === session) return this.#healthRefresh.operation;
    const operation = this.#refreshHealth(session).finally(() => {
      if (this.#healthRefresh?.operation === operation) this.#healthRefresh = null;
    });
    this.#healthRefresh = { session, operation };
    return operation;
  }

  getAcceptanceEndpointObservability(): Promise<HelperAcceptanceEndpointObservability> {
    if (this.#options.platform !== 'win32') {
      return Promise.reject(
        new HelperClientError(
          'not-running',
          'Acceptance endpoint observability is unavailable on this platform',
        ),
      );
    }
    return this.request('acceptance.endpoint_observability', {});
  }

  pauseAcceptanceLeaseRenewal(): Promise<HelperAcceptancePauseLeaseRenewalResult> {
    const session = this.#rpcSession;
    if (
      this.#options.platform !== 'win32' ||
      session === null ||
      !this.#ordinaryRequestsAvailable()
    ) {
      return Promise.reject(
        new HelperClientError('not-running', 'Acceptance lease-renewal pause is unavailable'),
      );
    }
    return this.#rpcChannel.request(
      session,
      'acceptance.pause_lease_renewal',
      {},
      {
        timeoutMs: ACCEPTANCE_LEASE_RENEWAL_TIMEOUT_MS,
        timeoutReason: 'request-timeout',
        allowDraining: false,
        supervision: false,
      },
    );
  }

  async getRuntimeObservability(): Promise<HelperRuntimeObservability> {
    const observability = await this.request('runtime.observability', {});
    const enriched = HelperRuntimeObservabilitySchema.parse({
      ...observability,
      registeredInput: {
        ...observability.registeredInput,
        electronReceived: this.#electronRegisteredObservations,
        observationAccepted: this.#physicalObservationsAccepted,
      },
    });
    this.#publishRuntimeObservability(enriched, 'runtime');
    return enriched;
  }

  /** Records only successful non-activating physical-observation acceptance. */
  recordPhysicalObservationAccepted(): void {
    this.#physicalObservationsAccepted = saturatingSafeIncrement(
      this.#physicalObservationsAccepted,
    );
  }

  async prepareOwnerMaintenance(
    params: HelperPrepareMaintenanceParams,
    timeoutMs: number,
    signal?: AbortSignal,
  ) {
    if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
      throw new HelperClientError(
        'request-timeout',
        'Native helper maintenance deadline is invalid',
      );
    }
    const session = this.#rpcSession;
    if (!this.#ordinaryRequestsAvailable() || session === null) {
      throw new HelperClientError('not-running', 'Native helper is terminating');
    }
    const deadlineAt = performance.now() + timeoutMs;
    this.#maintenancePreparing = true;
    this.#clearHeartbeat();
    const dispatchState = { dispatched: false };
    let result: HelperResult<'owner.prepare_maintenance'>;
    try {
      const remaining = deadlineAt - performance.now();
      if (remaining <= 0) {
        throw new HelperClientError(
          'request-timeout',
          'Native helper maintenance deadline expired',
        );
      }
      result = await this.#rpcChannel.request(session, 'owner.prepare_maintenance', params, {
        timeoutMs: remaining,
        timeoutReason: 'owner-draining',
        signal,
        allowDraining: false,
        supervision: true,
        timeoutStartsOnDispatch: false,
        onDispatched: () => {
          dispatchState.dispatched = true;
        },
      });
    } catch (error: unknown) {
      if (!dispatchState.dispatched) {
        this.#maintenancePreparing = false;
        const child = this.#child;
        if (child !== null && this.#ordinaryRequestsAvailable()) {
          this.#startHeartbeat(child, session);
        }
      } else if (this.#rpcSession === session) {
        const reason =
          error instanceof HelperClientError
            ? (ownerReasonFromRpcCode(error.rpcCode) ?? 'owner-indeterminate')
            : 'owner-indeterminate';
        this.#terminateCurrent(reason, false, error instanceof Error ? error : undefined);
      }
      throw error;
    }
    if (this.#rpcSession !== session || !this.#rpcChannel.isCurrent(session)) {
      throw new HelperClientError('not-running', 'Native helper maintenance changed process');
    }
    this.#maintenancePreparing = false;
    this.#maintenancePrepared = true;
    this.#sessionAuthoritative = false;
    this.#captureDisabledForSession = true;
    this.#sessionKeyCaptureAvailable = false;
    this.#activation.processUnavailable(session);
    this.#setReadiness({
      status: 'unavailable',
      reason: 'owner-maintenance',
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    this.#rpcChannel.beginDraining(session);
    const stopping = this.#stopCurrentProcess();
    const stopped = await waitForOutcomeWithin(
      stopping,
      Math.max(0, deadlineAt - performance.now()),
    );
    if (!stopped.completed) {
      throw new HelperClientError('request-timeout', 'Native helper maintenance exit timed out');
    }
    if (stopped.error !== null) {
      throw stopped.error instanceof Error
        ? stopped.error
        : new HelperClientError('transport-error', 'Native helper maintenance exit failed');
    }
    return result;
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
    if (
      session === null ||
      (!allowStopping && !this.#ordinaryRequestsAvailable()) ||
      (!this.#desiredRunning && !allowStopping)
    ) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is terminating'));
    }
    return this.#rpcChannel.request(session, method, params, {
      timeoutMs: Math.min(timeoutMs, REQUEST_TIMEOUT_MS),
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
    // Fence ordinary work and notifications behind this authenticated owner snapshot.
    // Requests already dispatched remain ordered before the health check. One gesture from the
    // same authenticated owner may be replayed after a successful disabled-first reconciliation.
    this.#sessionAuthoritative = false;
    this.#pendingActivationPolicy = 'replay-current-gesture';
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
    if (!this.#ownerAssociationMatches(health.keyboardOwner)) {
      await this.#reconcileLocalOwnerChange(
        session,
        health.keyboardOwner,
        health.hookStatus,
        permissions,
      );
      return permissions;
    }
    this.#sessionAuthoritative = true;
    const readiness = readinessFromOwner(
      this.#readiness.helperVersion,
      health.hookStatus,
      permissions,
      health.keyboardOwner,
      this.#captureDisabledForSession,
      this.#runtimeRollbackForSession,
    );
    this.#activation.setBlockedByHealth(readiness.status !== 'ready');
    const ownerUnavailable = readiness.reason?.startsWith('owner-') === true;
    if (this.#captureDisabledForSession || ownerUnavailable) {
      this.#pendingAuthoritativeActivation.length = 0;
      this.#pendingActivationPolicy = 'drop';
      this.#sessionAuthoritative = false;
      this.#setReadiness(readiness);
      return permissions;
    }
    try {
      await this.#activation.reconcileSession(session, false);
    } catch (error: unknown) {
      if (!this.#healthSessionIsActive(session)) return permissions;
      this.#terminateCurrent(readiness.reason ?? 'hook-fault', true);
      throw error;
    }
    if (!this.#healthSessionIsActive(session)) return permissions;
    this.#setReadiness(readiness);
    this.#flushAuthoritativeActivation();
    if (
      readiness.reason === 'hook-fault' &&
      readiness.status === 'unavailable' &&
      permissionsAreGranted(permissions) &&
      !hookTransportReady(health.hookStatus)
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

  async #reconcileLocalOwnerChange(
    session: HelperRpcSession,
    owner: HelperKeyboardOwnerSnapshot,
    hookStatus: HelperInitializeResult['hookStatus'],
    permissions: HelperPermissions,
  ): Promise<void> {
    const previous = this.#ownerAssociation;
    if (!owner.authenticated || owner.leaseEpoch === null) {
      const reason: HelperReadinessReason =
        owner.state === 'unavailable' ? 'owner-missing' : 'owner-auth-failed';
      this.#pendingAuthoritativeActivation.length = 0;
      this.#pendingActivationPolicy = 'drop';
      this.#sessionAuthoritative = false;
      this.#activation.processUnavailable(session);
      this.#setReadiness({
        status: 'unavailable',
        reason,
        helperVersion: this.#readiness.helperVersion,
        permissions,
      });
      if (reason === 'owner-auth-failed') this.#terminateCurrent(reason, false);
      return;
    }
    if (previous === null || (previous.buildId !== '' && owner.buildId !== previous.buildId)) {
      this.#pendingActivationPolicy = 'drop';
      this.#terminateCurrent('owner-degraded', true);
      throw new HelperClientError('rpc-error', 'Native helper owner association changed');
    }

    this.#sessionAuthoritative = false;
    this.#pendingActivationPolicy = 'replay-current-gesture';
    this.#setReadiness({
      status: 'starting',
      reason: null,
      helperVersion: this.#readiness.helperVersion,
      permissions,
    });
    this.#ownerAssociation = {
      instanceId: owner.instanceId,
      buildId: owner.buildId,
      leaseEpoch: owner.leaseEpoch,
    };
    this.#captureDisabledForSession =
      this.#captureBuildDisabledForSession || this.#runtimeRollbackForSession;
    this.#sessionKeyCaptureAvailable = !this.#captureDisabledForSession;
    this.#rpcChannel.resetOwnerActivationStream(session);
    this.#activation.prepareFreshSession();
    const readiness = readinessFromOwner(
      this.#readiness.helperVersion,
      hookStatus,
      permissions,
      owner,
      this.#captureDisabledForSession,
      this.#runtimeRollbackForSession,
    );
    this.#activation.setBlockedByHealth(readiness.status !== 'ready');
    try {
      const capture = await this.#rpcChannel.request(
        session,
        'session.set_capture',
        { mode: 'off' },
        {
          timeoutMs: REQUEST_TIMEOUT_MS,
          timeoutReason: 'request-timeout',
          allowDraining: false,
          supervision: true,
        },
      );
      if (capture.mode !== 'off') {
        throw new HelperClientError(
          'rpc-error',
          'Native helper did not confirm disabled session capture',
        );
      }
      await this.#activation.reconcileFreshHelper(
        session,
        REQUEST_TIMEOUT_MS,
        'request-timeout',
        () => {
          if (this.#rpcSession === session && this.#rpcChannel.isCurrent(session)) {
            this.#sessionAuthoritative = true;
          }
        },
      );
    } catch (error: unknown) {
      if (this.#healthSessionIsActive(session)) this.#terminateCurrent('owner-degraded', true);
      throw error;
    }
    if (!this.#healthSessionIsActive(session)) return;
    this.#setReadiness(readiness);
    this.#flushAuthoritativeActivation();
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
    if (this.#crashLoopOpen) this.#halfOpenProbe = true;
    this.#launching = this.#launch().finally(() => {
      this.#launching = null;
    });
    await this.#launching;
  }

  async #stopCurrentProcess(requireNeutral = false): Promise<void> {
    if (this.#stopOperation !== null) return this.#stopOperation;
    this.#stopTerminalFault = null;
    const session = this.#rpcSession;
    if (session !== null) this.#rpcChannel.beginDraining(session);
    const operation = Promise.resolve().then(() => this.#performStop(requireNeutral));
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

  async #requestBoundedShutdown(session: HelperRpcSession): Promise<{
    readonly dispatchAt: number | null;
    readonly succeeded: boolean;
    readonly ownerDisposition: 'neutral' | 'draining' | null;
    readonly error: unknown;
  }> {
    const abort = new AbortController();
    let resolveDispatched: (dispatchedAt: number) => void = () => undefined;
    const dispatched = new Promise<number>((resolve) => {
      resolveDispatched = resolve;
    });
    const completion = this.#rpcChannel.request(
      session,
      'shutdown',
      {},
      {
        timeoutMs: this.#shutdownWaitMs,
        timeoutReason: 'request-timeout',
        signal: abort.signal,
        allowDraining: true,
        supervision: true,
        timeoutStartsOnDispatch: true,
        onDispatched: () => resolveDispatched(performance.now()),
      },
    );
    const outcome = completion.then(
      (value) => ({
        kind: 'completed' as const,
        ownerDisposition: value.ownerDisposition,
        error: null,
      }),
      (error: unknown) => ({ kind: 'failed' as const, error }),
    );
    const dispatch = await waitForValueWithin(dispatched, this.#predecessorDrainWaitMs);
    if (!dispatch.completed) {
      abort.abort();
      await outcome;
      return {
        dispatchAt: null,
        succeeded: false,
        ownerDisposition: null,
        error: new HelperClientError(
          'request-timeout',
          'Native helper predecessor drain timed out',
        ),
      };
    }
    const result = await outcome;
    return {
      dispatchAt: dispatch.value,
      succeeded: result.kind === 'completed',
      ownerDisposition: result.kind === 'completed' ? result.ownerDisposition : null,
      error: result.error,
    };
  }

  async #performStop(requireNeutral: boolean): Promise<void> {
    this.#clearRestart();
    this.#clearHeartbeat();
    const child = this.#child;
    if (child !== null) {
      this.#plannedExit = { reason: 'shutdown', restart: false, lifecycle: true };
      this.#terminating = true;
      const close = waitForClose(child);
      const session = this.#rpcSession;
      let shutdownError: unknown = null;
      let shutdownSucceeded = false;
      let shutdownDispatchedAt: number | null = null;
      let ownerDisposition: 'neutral' | 'draining' | null = null;
      try {
        if (session !== null && this.#rpcChannel.isCurrent(session)) {
          const shutdown = await this.#requestBoundedShutdown(session);
          shutdownDispatchedAt = shutdown.dispatchAt;
          shutdownSucceeded = shutdown.succeeded;
          ownerDisposition = shutdown.ownerDisposition;
          shutdownError = shutdown.error;
        }
        const remaining =
          shutdownDispatchedAt === null
            ? 0
            : Math.max(0, this.#shutdownWaitMs - (performance.now() - shutdownDispatchedAt));
        let closed = await waitForCloseWithin(close.promise, remaining);
        if (!closed && shutdownError === null) {
          closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
        }
        if (!closed && this.#child === child) {
          if (session !== null && this.#rpcSession === session) {
            this.#rpcChannel.close(
              session,
              new HelperClientError('transport-error', 'Native helper shutdown timed out'),
            );
            this.#activation.processUnavailable(session);
          }
          child.kill();
          closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
        }
        if (!closed && this.#child === child) {
          throw new HelperClientError(
            'transport-error',
            'Native helper exit could not be confirmed',
          );
        }
        if (
          this.#stopTerminalFault !== null ||
          !shutdownSucceeded ||
          child.exitCode !== 0 ||
          (requireNeutral && ownerDisposition !== 'neutral')
        ) {
          throw (
            this.#stopTerminalFault ??
            (shutdownError instanceof Error
              ? shutdownError
              : new HelperClientError(
                  'transport-error',
                  'Native helper did not complete clean shutdown',
                ))
          );
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
      if (this.#halfOpenProbe) {
        this.#recordFailure(reason);
        return;
      }
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
        env: Object.freeze({
          ...process.env,
          NO_COLOR: '1',
          [ACTIVATION_CAPTURE_ROLLBACK_ENV]:
            this.#options.disableActivationCapture === true ? '1' : undefined,
          [OWNER_DIAGNOSTIC_JOURNAL_ENV]: this.#options.diagnosticJournalPath,
        }),
        shell: false,
        detached: this.#options.platform === 'win32',
        windowsHide: true,
      });
    } catch {
      this.#recordFailure('spawn-failed');
      return;
    }
    this.#child = child;
    const session = this.#rpcChannel.attach(child);
    this.#rpcSession = session;
    this.#sessionAuthoritative = false;
    this.#maintenancePreparing = false;
    this.#maintenancePrepared = false;
    this.#captureDisabledForSession = false;
    this.#captureBuildDisabledForSession = false;
    this.#runtimeRollbackForSession = false;
    this.#sessionKeyCaptureAvailable = null;
    this.#ownerAssociation = null;
    this.#nativeLaunchFailure = null;
    this.#activation.prepareFreshSession();
    this.#attachProcess(child, session, generation);
    child.once('spawn', () => this.#publishProcessLifecycle({ phase: 'started' }));

    const launchDeadline = performance.now() + HANDSHAKE_TIMEOUT_MS;
    const remainingLaunchTime = () => launchDeadline - performance.now();
    let launchOwnerReason: HelperReadinessReason | null = null;
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
      this.#ownerAssociation = {
        instanceId: initialized.keyboardOwner.instanceId,
        buildId: initialized.keyboardOwner.buildId,
        leaseEpoch: initialized.keyboardOwner.leaseEpoch,
      };
      this.#captureBuildDisabledForSession = initialized.keyboardCapture.buildDisabled;
      this.#captureDisabledForSession = !initialized.keyboardCapture.activationAvailable;
      this.#runtimeRollbackForSession = initialized.keyboardCapture.runtimeRollbackActive;
      this.#sessionKeyCaptureAvailable = initialized.keyboardCapture.sessionKeyCaptureAvailable;
      let readiness = readinessFromOwner(
        initialized.helperVersion,
        initialized.hookStatus,
        initialized.permissions,
        initialized.keyboardOwner,
        this.#captureDisabledForSession,
        initialized.keyboardCapture.runtimeRollbackActive,
      );
      launchOwnerReason = readiness.reason?.startsWith('owner-') === true ? readiness.reason : null;
      // A fresh helper is natively disabled. Keep it blocked until a second
      // health snapshot confirms that replaying retained activation is safe.
      this.#activation.setBlockedByHealth(true);
      {
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
        if (!this.#ownerAssociationMatches(health.keyboardOwner)) {
          throw new HelperClientError('rpc-error', 'Native helper owner association changed');
        }
        readiness = readinessFromOwner(
          initialized.helperVersion,
          health.hookStatus,
          permissions,
          health.keyboardOwner,
          this.#captureDisabledForSession,
          initialized.keyboardCapture.runtimeRollbackActive,
        );
        launchOwnerReason =
          readiness.reason?.startsWith('owner-') === true ? readiness.reason : null;
      }
      {
        const capture = await this.#rpcChannel.request(
          session,
          'session.set_capture',
          { mode: 'off' },
          {
            timeoutMs: remainingLaunchTime(),
            timeoutReason: 'handshake-timeout',
            allowDraining: false,
            supervision: true,
          },
        );
        if (capture.mode !== 'off') {
          throw new HelperClientError(
            'rpc-error',
            'Native helper did not confirm disabled session capture',
          );
        }
      }
      if (readiness.status === 'ready' && !this.#captureDisabledForSession) {
        this.#activation.setBlockedByHealth(false);
      }
      await this.#activation.reconcileFreshHelper(
        session,
        remainingLaunchTime(),
        'handshake-timeout',
        () => {
          if (this.#rpcSession === session && this.#rpcChannel.isCurrent(session)) {
            this.#sessionAuthoritative = true;
          }
        },
      );
      if (this.#captureDisabledForSession) this.#activation.setBlockedByHealth(true);
      if (!this.#isActiveChild(child, session)) return;
      if (this.#halfOpenProbe) {
        this.#failureTimes = [];
        this.#crashLoopOpen = false;
        this.#halfOpenProbe = false;
      }
      this.#setReadiness(readiness);
      this.#flushAuthoritativeActivation();
      this.#startHeartbeat(child, session);
    } catch (error) {
      if (this.#child !== child || this.#rpcSession !== session) return;
      const fallbackReason = classifyLaunchError(error, launchOwnerReason);
      await this.#waitForStartupStderr(generation);
      if (!this.#isActiveChild(child, session)) return;
      const reason =
        readinessReasonFromNativeLaunchFailure(this.#nativeLaunchFailure) ?? fallbackReason;
      this.#terminateCurrent(reason, shouldRestartAfterFailure(reason));
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
    // Stderr is never copied into diagnostics. Only the helper's strict,
    // aggregate-only terminal record can cross into the privacy-safe sink.
    const stderrDecoder = new StringDecoder('utf8');
    let stderrLine = '';
    let discardOversizedLine = false;
    let terminalCandidate: HelperTerminalObservabilityRecord | null = null;
    let duplicateTerminalCandidate = false;
    let resolveStderrDrain: () => void = () => undefined;
    const stderrDrain = {
      generation,
      promise: new Promise<void>((resolve) => {
        resolveStderrDrain = resolve;
      }),
      complete: () => {
        if (stderrDrain.completed) return;
        stderrDrain.completed = true;
        resolveStderrDrain();
      },
      completed: false,
    };
    this.#stderrDrain = stderrDrain;
    const acceptStderrLine = (): void => {
      const line = stderrLine.endsWith('\r') ? stderrLine.slice(0, -1) : stderrLine;
      const parsed = discardOversizedLine ? null : this.#parseDiagnosticLine(line);
      if (parsed?.kind === 'terminal') {
        if (terminalCandidate === null) terminalCandidate = parsed.value;
        else duplicateTerminalCandidate = true;
      } else if (parsed?.kind === 'owner-connection') {
        this.#commitAndAcknowledgeOwnerDiagnostic(session, parsed.value);
      }
      const launchFailure = classifyNativeLaunchFailure(line);
      if (launchFailure !== null) this.#nativeLaunchFailure ??= launchFailure;
      stderrLine = '';
      discardOversizedLine = false;
    };
    const consumeStderr = (decoded: string): void => {
      for (const fragment of decoded.split(/(\n)/u)) {
        if (fragment === '\n') {
          acceptStderrLine();
          continue;
        }
        if (discardOversizedLine) continue;
        stderrLine += fragment;
        if (Buffer.byteLength(stderrLine) > MAX_TERMINAL_OBSERVABILITY_LINE_BYTES) {
          stderrLine = '';
          discardOversizedLine = true;
        }
      }
    };
    const finishStderr = (): void => {
      if (stderrDrain.completed) return;
      consumeStderr(stderrDecoder.end());
      if (stderrLine !== '' || discardOversizedLine) acceptStderrLine();
      stderrDrain.complete();
    };
    child.stderr.once('error', finishStderr);
    child.stderr.once('end', finishStderr);
    child.stderr.once('close', finishStderr);
    child.stderr.on('data', (chunk: Buffer | string) => {
      consumeStderr(typeof chunk === 'string' ? chunk : stderrDecoder.write(Buffer.from(chunk)));
    });
    child.once('error', () => {
      if (this.#child === child && this.#rpcSession === session) {
        this.#terminateCurrent('spawn-failed', true);
      }
    });
    child.once('close', (code: number | null, signal: NodeJS.Signals | null) => {
      stderrDrain.complete();
      this.#publishProcessLifecycle({
        phase: 'exited',
        exitCode: code,
        signal,
        planned: this.#plannedExit?.lifecycle === true || !this.#desiredRunning,
      });
      const exitDiagnostic = safeChildExitDiagnostic(code, signal);
      if (this.#launching !== null && this.#nativeLaunchFailure === null) {
        this.#nativeLaunchFailure = exitDiagnostic;
      }
      const expectedOutcome = code === 0 && signal === null ? 'shutdown' : 'failure';
      if (
        !duplicateTerminalCandidate &&
        terminalCandidate !== null &&
        terminalCandidate.outcome === expectedOutcome
      ) {
        this.#publishRuntimeObservability(
          terminalCandidate.observability,
          terminalCandidate.outcome,
        );
      }
      this.#handleClose(child, session, generation);
    });
  }

  async #waitForStartupStderr(generation: number): Promise<void> {
    const drain = this.#stderrDrain;
    if (drain?.generation !== generation || drain.completed) return;
    await waitForCloseWithin(drain.promise, STARTUP_STDERR_DRAIN_MS);
  }

  #parseDiagnosticLine(
    line: string,
  ):
    | { readonly kind: 'terminal'; readonly value: HelperTerminalObservabilityRecord }
    | { readonly kind: 'owner-connection'; readonly value: HelperOwnerConnectionDiagnostic }
    | null {
    let candidate: unknown;
    try {
      candidate = JSON.parse(line);
    } catch {
      return null;
    }
    const terminal = HelperTerminalObservabilityRecordSchema.safeParse(candidate);
    if (terminal.success) return { kind: 'terminal', value: terminal.data };
    const ownerConnection = HelperOwnerConnectionDiagnosticSchema.safeParse(candidate);
    return ownerConnection.success
      ? { kind: 'owner-connection', value: ownerConnection.data }
      : null;
  }

  #commitAndAcknowledgeOwnerDiagnostic(
    session: HelperRpcSession,
    diagnostic: HelperOwnerConnectionDiagnostic,
  ): void {
    const observer = this.#options.observeOwnerConnectionDiagnostic;
    if (observer === undefined || !this.#rpcChannel.isCurrent(session)) return;
    const dimensions = {
      category: diagnostic.category,
      operation: diagnostic.operation,
      correlationStatus: diagnostic.correlationStatus,
      healthRefresh: diagnostic.healthRefresh,
      transportStatus: diagnostic.transportStatus,
      ownerProcessState: diagnostic.ownerProcessState,
    } as const;
    const key = JSON.stringify({
      journalId: diagnostic.journalId,
      journalNonce: diagnostic.journalNonce,
      dimensions,
      count: diagnostic.count,
    });
    if (this.#diagnosticAcksInFlight.has(key)) return;
    if (this.#diagnosticAcksInFlight.size >= MAX_DIAGNOSTIC_ACKS_IN_FLIGHT) return;
    const operation = Promise.resolve()
      .then(() => observer(diagnostic))
      .then(async (committed) => {
        if (!committed || !this.#rpcChannel.isCurrent(session)) return;
        await this.#rpcChannel.request(
          session,
          'diagnostic.ack',
          {
            journalId: diagnostic.journalId,
            journalNonce: diagnostic.journalNonce,
            dimensions,
            count: diagnostic.count,
          },
          {
            timeoutMs: REQUEST_TIMEOUT_MS,
            timeoutReason: 'request-timeout',
            allowDraining: false,
            supervision: false,
            priority: false,
          },
        );
      })
      .catch(() => undefined)
      .finally(() => this.#diagnosticAcksInFlight.delete(key));
    this.#diagnosticAcksInFlight.set(key, operation);
  }

  #publishProcessLifecycle(event: {
    readonly phase: 'started' | 'exited';
    readonly exitCode?: number | null;
    readonly signal?: NodeJS.Signals | null;
    readonly planned?: boolean;
  }): void {
    try {
      void Promise.resolve(this.#options.observeProcessLifecycle?.(event)).catch(() => undefined);
    } catch {
      // Diagnostics cannot affect native supervision.
    }
  }

  #publishRuntimeObservability(
    observability: HelperRuntimeObservability,
    source: HelperRuntimeObservabilitySource,
  ): void {
    const observer = this.#options.observeRuntimeObservability;
    if (observer === undefined) return;
    try {
      void Promise.resolve(observer(observability, source)).catch(() => undefined);
    } catch {
      // Diagnostics cannot affect native supervision.
    }
  }

  #handleRpcFault(
    session: HelperRpcSession,
    reason: HelperReadinessReason,
    pendingError?: Error,
  ): void {
    if (this.#rpcSession !== session) return;
    // During startup the rejected request is only one half of the process
    // outcome. Coordinate it with bounded stderr drainage before selecting
    // retry policy and publishing readiness. The RPC channel intentionally
    // leaves pending requests open until supervision closes it.
    if (this.#launching !== null && this.#stopOperation === null) {
      const generation = this.#generation;
      void (async () => {
        await this.#waitForStartupStderr(generation);
        if (this.#rpcSession !== session || this.#generation !== generation) return;
        const startupReason =
          readinessReasonFromNativeLaunchFailure(this.#nativeLaunchFailure) ?? reason;
        this.#terminateCurrent(
          startupReason,
          shouldRestartAfterFailure(startupReason),
          pendingError,
        );
      })();
      return;
    }
    const effectiveReason =
      readinessReasonFromNativeLaunchFailure(this.#nativeLaunchFailure) ?? reason;
    if (this.#stopOperation !== null) {
      if (reason !== 'malformed-response') return;
      this.#stopTerminalFault ??=
        pendingError ??
        new HelperClientError('transport-error', 'Native helper returned malformed output');
      this.#setReadiness({
        status: 'unavailable',
        reason,
        helperVersion: this.#readiness.helperVersion,
        permissions: this.#readiness.permissions,
      });
      // The channel released malformed predecessor authority without pumping.
      // Drain policy is authoritative now, so only reserved shutdown may run.
      this.#rpcChannel.beginDraining(session);
      return;
    }
    this.#terminateCurrent(
      effectiveReason,
      shouldRestartAfterFailure(effectiveReason),
      pendingError,
    );
  }

  #publishNotification(session: HelperRpcSession, notification: HelperNotification): void {
    if (this.#rpcSession !== session || this.#ownerAssociation === null) return;
    if (!this.#sessionAuthoritative) {
      if (notification.method === 'activation.event') {
        this.#queueAuthoritativeActivation(notification);
      }
      return;
    }
    this.#publishAuthoritativeNotification(notification);
  }

  #publishAuthoritativeNotification(notification: HelperNotification): void {
    if (
      (notification.method === 'activation.event' && this.#captureDisabledForSession) ||
      (notification.method === 'session.key' && this.#sessionKeyCaptureAvailable === false)
    ) {
      // The process-lifetime capability is authoritative. Ignore impossible
      // keyboard notifications rather than arming or retrying unsupported
      // native capture in application reconciliation.
      return;
    }
    if (
      notification.method === 'registered_input.observed' ||
      notification.method === 'activation.event'
    ) {
      this.#electronRegisteredObservations = saturatingSafeIncrement(
        this.#electronRegisteredObservations,
      );
    }
    for (const listener of this.#notificationListeners) {
      try {
        listener(notification);
      } catch {
        // Native protocol health must not depend on a consumer callback.
      }
    }
  }

  #queueAuthoritativeActivation(
    notification: Extract<HelperNotification, { method: 'activation.event' }>,
  ): void {
    if (this.#pendingActivationPolicy === 'drop') return;
    if (notification.params.phase !== 'up') {
      // The latest valid start supersedes an older unbalanced candidate. This keeps a bounded
      // queue while allowing down(1), down(2), up(2) to replay the current gesture exactly.
      this.#pendingAuthoritativeActivation = [notification];
      return;
    }
    const first = this.#pendingAuthoritativeActivation[0];
    if (
      first?.params.phase === 'down' &&
      notification.params.activationGeneration === first.params.activationGeneration &&
      notification.params.targetToken === first.params.targetToken
    ) {
      this.#pendingAuthoritativeActivation = [first, notification];
    }
  }

  #flushAuthoritativeActivation(): void {
    if (!this.#sessionAuthoritative || this.#readiness.status !== 'ready') return;
    const pending = this.#pendingAuthoritativeActivation.splice(0);
    this.#pendingActivationPolicy = 'drop';
    for (const notification of pending) {
      this.#publishAuthoritativeNotification(notification);
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
    this.#sessionAuthoritative = false;
    this.#maintenancePreparing = false;
    this.#maintenancePrepared = false;
    this.#captureDisabledForSession = false;
    this.#captureBuildDisabledForSession = false;
    this.#runtimeRollbackForSession = false;
    this.#sessionKeyCaptureAvailable = null;
    this.#ownerAssociation = null;
    this.#pendingAuthoritativeActivation.length = 0;
    this.#pendingActivationPolicy = 'drop';
    this.#terminating = false;
    this.#clearHeartbeat();
    const planned = this.#plannedExit;
    this.#plannedExit = null;
    if (!this.#desiredRunning || this.#stopOperation !== null) return;
    const nativeReason = readinessReasonFromNativeLaunchFailure(this.#nativeLaunchFailure);
    const reason = nativeReason ?? planned?.reason ?? 'unexpected-exit';
    this.#recordFailure(
      reason,
      nativeReason === null ? (planned?.restart ?? true) : shouldRestartAfterFailure(nativeReason),
    );
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
    this.#plannedExit ??= { reason, restart, lifecycle: false };
    this.#pendingAuthoritativeActivation.length = 0;
    this.#pendingActivationPolicy = 'drop';
    this.#activation.processUnavailable(session);
    this.#clearHeartbeat();
    this.#setReadiness({
      status:
        !this.#desiredRunning && reason === 'shutdown'
          ? 'stopped'
          : reason === 'owner-incompatible'
            ? 'incompatible'
            : 'unavailable',
      reason,
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    this.#rpcChannel.beginDraining(session);
    const close = waitForClose(child);
    void (async () => {
      try {
        const shutdown = await this.#requestBoundedShutdown(session);
        const remaining =
          shutdown.dispatchAt === null
            ? 0
            : Math.max(0, this.#shutdownWaitMs - (performance.now() - shutdown.dispatchAt));
        let closed = await waitForCloseWithin(close.promise, remaining);
        if (!closed && this.#child === child) {
          if (this.#rpcSession === session) this.#rpcChannel.close(session, pendingError);
          child.kill();
          closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
        }
        if (!closed && this.#child === child) {
          this.#setReadiness({
            status: 'unavailable',
            reason: 'hook-fault',
            helperVersion: this.#readiness.helperVersion,
            permissions: this.#readiness.permissions,
          });
        }
      } finally {
        close.cancel();
      }
    })();
  }

  #recordFailure(reason: HelperReadinessReason, restart = true): void {
    if (isOwnerTransition(reason)) {
      // A predecessor may retain the owner singleton while it drains held key
      // state. Keep this outside crash accounting so normal handoff cannot
      // open Electron's two-minute crash-loop circuit.
      this.#failureTimes = [];
      this.#crashLoopOpen = false;
      this.#halfOpenProbe = false;
      this.#setReadiness({
        status: 'unavailable',
        reason,
        helperVersion: this.#readiness.helperVersion,
        permissions: this.#readiness.permissions,
      });
      if (restart) this.#scheduleRestart(RESTART_DELAYS_MS[0]);
      else this.#clearRestart();
      return;
    }
    if (!restart) {
      this.#failureTimes = [];
      this.#crashLoopOpen = false;
      this.#halfOpenProbe = false;
      this.#clearRestart();
      this.#setReadiness({
        status:
          reason === 'protocol-mismatch' || reason === 'owner-incompatible'
            ? 'incompatible'
            : 'unavailable',
        reason,
        helperVersion: this.#readiness.helperVersion,
        permissions: this.#readiness.permissions,
      });
      return;
    }

    const now = Date.now();
    if (this.#halfOpenProbe) {
      this.#openCrashLoop();
      return;
    }
    this.#failureTimes = this.#failureTimes.filter((time) => now - time < FAILURE_WINDOW_MS);
    this.#failureTimes.push(now);
    if (this.#failureTimes.length >= FAILURE_LIMIT) {
      this.#openCrashLoop();
      return;
    }

    this.#setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    const delayIndex = Math.min(this.#failureTimes.length - 1, RESTART_DELAYS_MS.length - 1);
    this.#scheduleRestart(RESTART_DELAYS_MS[delayIndex] ?? RESTART_DELAYS_MS[0]);
  }

  #openCrashLoop(): void {
    this.#failureTimes = [];
    this.#crashLoopOpen = true;
    this.#halfOpenProbe = false;
    this.#setReadiness({
      status: 'unavailable',
      reason: 'crash-loop',
      helperVersion: this.#readiness.helperVersion,
      permissions: this.#readiness.permissions,
    });
    this.#scheduleRestart(FAILURE_WINDOW_MS);
  }

  #scheduleRestart(restartAfter: number): void {
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
      void this.#refreshHealthCoalesced(session).catch((error: unknown) => {
        if (error instanceof HelperClientError && error.code === 'request-capacity') return;
        if (this.#rpcSession === session && this.#desiredRunning && this.#stopOperation === null) {
          const reason = classifyLaunchError(error);
          if (isOwnerTransition(reason)) {
            this.#pendingAuthoritativeActivation.length = 0;
            this.#pendingActivationPolicy = 'drop';
            this.#setReadiness({
              status: 'unavailable',
              reason,
              helperVersion: this.#readiness.helperVersion,
              permissions: this.#readiness.permissions,
            });
            return;
          }
          this.#terminateCurrent(
            error instanceof HelperClientError && error.code === 'request-timeout'
              ? 'request-timeout'
              : reason,
            shouldRestartAfterFailure(reason),
          );
        }
      });
    }, HEARTBEAT_INTERVAL_MS);
    this.#heartbeatTimer.unref();
  }

  #ordinaryRequestsAvailable(): boolean {
    return (
      this.#sessionAuthoritative &&
      !this.#maintenancePreparing &&
      !this.#maintenancePrepared &&
      !this.#terminating &&
      this.#desiredRunning &&
      this.#stopOperation === null
    );
  }

  #ownerAssociationMatches(owner: HelperKeyboardOwnerSnapshot): boolean {
    return (
      this.#ownerAssociation !== null &&
      owner.instanceId === this.#ownerAssociation.instanceId &&
      owner.buildId === this.#ownerAssociation.buildId &&
      owner.leaseEpoch === this.#ownerAssociation.leaseEpoch
    );
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

function saturatingSafeIncrement(value: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, value + 1);
}

function defaultSpawnHelper(
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
): ChildProcessWithoutNullStreams {
  return spawn(executablePath, [], {
    ...options,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
}

function readinessFromOwner(
  helperVersion: string | null,
  hookStatus: HelperInitializeResult['hookStatus'],
  permissions: HelperPermissions,
  owner: HelperKeyboardOwnerSnapshot,
  captureDisabled: boolean,
  runtimeRollbackActive: boolean,
): HelperReadiness {
  const unavailable = (reason: HelperReadinessReason, incompatible = false): HelperReadiness => ({
    status: incompatible ? 'incompatible' : 'unavailable',
    reason,
    helperVersion,
    permissions,
  });
  if (runtimeRollbackActive) return unavailable('owner-rollback');
  if (!owner.authenticated || owner.leaseEpoch === null) {
    return unavailable(owner.state === 'unavailable' ? 'owner-missing' : 'owner-auth-failed');
  }
  if (owner.state === 'draining') return unavailable('owner-draining');
  if (owner.state === 'maintenance') return unavailable('owner-maintenance');
  if (owner.state === 'degraded') return unavailable('owner-degraded');
  if (owner.state === 'unavailable') return unavailable('owner-missing');
  if (owner.state === 'idle') return unavailable('owner-busy');
  if (owner.state === 'safe_disabled' || captureDisabled) return unavailable('capture-disabled');

  return readinessFromHandshake(helperVersion, hookStatus, permissions);
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
  if (!hookTransportReady(hookStatus)) {
    return { status: 'unavailable', reason: 'hook-fault', helperVersion, permissions };
  }
  // `ready` means the authenticated transport, owner protocol, hook
  // installation, and message pump are available. Physical callback delivery
  // is reported independently by hookStatus/registered-input observability.
  return { status: 'ready', reason: null, helperVersion, permissions };
}

function hookTransportReady(hookStatus: HelperInitializeResult['hookStatus']): boolean {
  return hookStatus === 'installed_unobserved' || hookStatus === 'physical_observed';
}

function permissionsAreGranted(permissions: HelperPermissions): boolean {
  return Object.values(permissions).every(
    (permission) => permission === 'granted' || permission === 'not_applicable',
  );
}

function classifyLaunchError(
  error: unknown,
  ownerFallback: HelperReadinessReason | null = null,
): HelperReadinessReason {
  if (error instanceof HelperClientError) {
    if (error.code === 'request-timeout') return 'handshake-timeout';
    if (error.code === 'rpc-error') {
      const ownerReason = ownerReasonFromRpcCode(error.rpcCode);
      if (ownerReason !== null) return ownerReason;
      if (error.rpcCode === -32_001) return 'protocol-mismatch';
      if (error.message.includes('owner association changed')) return 'owner-degraded';
      if (error.message.includes('incompatible')) return ownerFallback ?? 'protocol-mismatch';
      return ownerFallback ?? 'hook-fault';
    }
  }
  return ownerFallback ?? 'malformed-response';
}

function classifyNativeLaunchFailure(line: string): string | null {
  const hookInstall =
    /^keyboard-owner hook install unavailable: (module|access_denied|module_unavailable|native_unavailable)$/u.exec(
      line,
    );
  if (hookInstall?.[1] !== undefined) return `hook-install-${hookInstall[1]}`;
  const safeConnectFailures = new Map([
    ['talking-quill-helper: keyboard owner endpoint is unavailable', 'owner-unavailable'],
    ['talking-quill-helper: keyboard owner authentication failed', 'owner-authentication-failed'],
    ['talking-quill-helper: keyboard owner is incompatible', 'owner-incompatible'],
    ['talking-quill-helper: keyboard owner is busy or draining', 'owner-busy'],
  ]);
  return safeConnectFailures.get(line) ?? null;
}

function readinessReasonFromNativeLaunchFailure(
  failure: string | null,
): HelperReadinessReason | null {
  if (failure === null) return null;
  if (failure === 'owner-singleton-collision') return 'owner-singleton-collision';
  if (failure === 'owner-incompatible') return 'owner-incompatible';
  if (failure === 'owner-authentication-failed') return 'owner-auth-failed';
  if (failure === 'owner-busy') return 'owner-busy';
  if (failure === 'owner-unavailable') {
    return 'owner-missing';
  }
  if (failure.startsWith('hook-install-')) return 'hook-fault';
  return null;
}

function safeChildExitDiagnostic(code: number | null, signal: NodeJS.Signals | null): string {
  if (code !== null && Number.isSafeInteger(code)) return `helper-exit-code-${String(code)}`;
  if (signal !== null && /^SIG[A-Z0-9]+$/u.test(signal)) {
    return `helper-exit-signal-${signal.toLowerCase()}`;
  }
  return 'helper-exit-unknown';
}

function isOwnerTransition(reason: HelperReadinessReason): boolean {
  return reason === 'owner-busy' || reason === 'owner-draining';
}

function shouldRestartAfterFailure(reason: HelperReadinessReason): boolean {
  // These faults identify incompatible local binaries or an unsafe protocol stream.
  // Relaunching the same binaries cannot repair them and causes visible process churn.
  // All remaining reasons retain bounded transient supervision.
  return ![
    'protocol-mismatch',
    'malformed-response',
    'owner-incompatible',
    'owner-auth-failed',
    'owner-security-fault',
    'owner-rollback',
    'owner-indeterminate',
    'owner-singleton-collision',
  ].includes(reason);
}

function ownerReasonFromRpcCode(rpcCode: number | null): HelperReadinessReason | null {
  switch (rpcCode) {
    case -32_005:
      return 'owner-auth-failed';
    case -32_006:
      return 'owner-incompatible';
    case -32_007:
      return 'owner-busy';
    case -32_008:
      return 'owner-draining';
    case -32_009:
      return 'owner-rollback';
    case -32_010:
      return 'owner-security-fault';
    case -32_011:
      return 'owner-indeterminate';
    case -32_012:
      return 'owner-singleton-collision';
    default:
      return null;
  }
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

async function waitForOutcomeWithin(
  operation: Promise<void>,
  milliseconds: number,
): Promise<
  | { readonly completed: false; readonly error: null }
  | { readonly completed: true; readonly error: unknown }
> {
  let resolveTimeout: (value: { readonly completed: false; readonly error: null }) => void = () =>
    undefined;
  const timeout = new Promise<{ readonly completed: false; readonly error: null }>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(
    () => resolveTimeout({ completed: false, error: null }),
    Math.max(0, milliseconds),
  );
  timer.unref();
  try {
    return await Promise.race([
      operation.then(
        () => ({ completed: true as const, error: null }),
        (error: unknown) => ({ completed: true as const, error }),
      ),
      timeout,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function waitForValueWithin<Value>(
  operation: Promise<Value>,
  milliseconds: number,
): Promise<{ readonly completed: true; readonly value: Value } | { readonly completed: false }> {
  let resolveTimeout: (value: { readonly completed: false }) => void = () => undefined;
  const timeout = new Promise<{ readonly completed: false }>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(() => resolveTimeout({ completed: false }), milliseconds);
  timer.unref();
  try {
    return await Promise.race([
      operation.then((value) => ({ completed: true as const, value })),
      timeout,
    ]);
  } finally {
    clearTimeout(timer);
  }
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
