import { defaultSpawnHelper } from './helper-process';
import { HelperClientError } from './helper-client-error';
import { type ChildProcessWithoutNullStreams } from './helper-process';
import { isAbsolute } from 'node:path';
import { type HelperNotification, type HelperPermissions } from '../../shared/helper/protocol';
import {
  INITIAL_HELPER_READINESS,
  type HelperReadiness,
  type HelperReadinessReason,
} from '../../shared/schemas/helper-readiness';
import { HelperActivationReconciler } from './helper-activation-reconciler';
import { HelperRpcChannel, type HelperRpcSession } from './helper-rpc-channel';
import {
  MAC_SHUTDOWN_WAIT_MS,
  WINDOWS_SHUTDOWN_WAIT_MS,
  PREDECESSOR_DRAIN_WAIT_MS,
  type SpawnHelper,
  type HelperClientOptions,
} from './helper-client-options';
import {
  refreshHealthCoalesced,
  refreshHealth,
  healthSessionIsActive,
  reconcileLocalOwnerChange,
  ordinaryRequestsAvailable,
  ownerAssociationMatches,
} from './helper-client-health';
import { launch, validateHandshake, canLaunch, isActiveChild } from './helper-client-launch';
import {
  prepareOwnerMaintenance,
  startForIntent,
  stopCurrentProcess,
  intentIsCurrent,
  assertResetAllowed,
  requestBoundedShutdown,
  performStop,
} from './helper-client-shutdown';
import {
  attachProcess,
  waitForStartupStderr,
  parseDiagnosticLine,
  commitAndAcknowledgeOwnerDiagnostic,
  publishProcessLifecycle,
  publishRuntimeObservability,
} from './helper-client-diagnostics';
import {
  handleRpcFault,
  handleClose,
  terminateCurrent,
  recordFailure,
  openCrashLoop,
  scheduleRestart,
  startHeartbeat,
  clearRestart,
  clearHeartbeat,
} from './helper-client-supervision';
import {
  publishNotification,
  publishAuthoritativeNotification,
  queueAuthoritativeActivation,
  flushAuthoritativeActivation,
  setReadiness,
} from './helper-client-events';

/** Internal state shared by the lifecycle operations. Never exposed by HelperClient.
 * Operations use this same instance so asynchronous fences never copy process state.
 */
export class HelperClientRuntime {
  readonly options: HelperClientOptions;
  readonly spawnHelper: SpawnHelper;
  readonly rpcChannel: HelperRpcChannel;
  readonly activation: HelperActivationReconciler;
  readonly shutdownWaitMs: number;
  readonly predecessorDrainWaitMs: number;
  readonly readinessListeners = new Set<(readiness: HelperReadiness) => void>();
  readonly notificationListeners = new Set<(notification: HelperNotification) => void>();
  child: ChildProcessWithoutNullStreams | null = null;
  rpcSession: HelperRpcSession | null = null;
  terminating = false;
  generation = 0;
  runIntentRevision = 0;
  desiredRunning = false;
  healthRefresh: {
    readonly session: HelperRpcSession | null;
    readonly operation: Promise<HelperPermissions>;
  } | null = null;
  launching: Promise<void> | null = null;
  stopOperation: Promise<void> | null = null;
  stopTerminalFault: Error | null = null;
  restartTimer: NodeJS.Timeout | null = null;
  heartbeatTimer: NodeJS.Timeout | null = null;
  missingOwnerHealthChecks = 0;
  plannedExit: {
    readonly reason: HelperReadinessReason;
    readonly restart: boolean;
    readonly lifecycle: boolean;
  } | null = null;
  failureTimes: number[] = [];
  crashLoopOpen = false;
  halfOpenProbe = false;
  sessionAuthoritative = false;
  maintenancePreparing = false;
  maintenancePrepared = false;
  captureDisabledForSession = false;
  captureBuildDisabledForSession = false;
  runtimeRollbackForSession = false;
  sessionKeyCaptureAvailable: boolean | null = null;
  ownerAssociation: {
    readonly instanceId: string;
    readonly buildId: string;
    readonly leaseEpoch: number | null;
  } | null = null;
  readiness: HelperReadiness = INITIAL_HELPER_READINESS;
  nativeLaunchFailure: string | null = null;
  stderrDrain: {
    readonly generation: number;
    readonly promise: Promise<void>;
    readonly complete: () => void;
    completed: boolean;
  } | null = null;
  readonly diagnosticAcksInFlight = new Map<string, Promise<void>>();
  electronRegisteredObservations = 0;
  physicalObservationsAccepted = 0;
  pendingAuthoritativeActivation: Extract<HelperNotification, { method: 'activation.event' }>[] =
    [];
  // Gestures observed behind an owner/configuration fence have an explicit disposition. Transient
  // authenticated reconciliation may replay one current gesture; terminal and unauthenticated
  // states always discard it.
  pendingActivationPolicy: 'drop' | 'replay-current-gesture' = 'drop';

  constructor(options: HelperClientOptions) {
    if (
      options.diagnosticJournalPath !== undefined &&
      (!isAbsolute(options.diagnosticJournalPath) || options.diagnosticJournalPath.includes('\0'))
    ) {
      throw new Error('Helper diagnostic journal path must be absolute');
    }
    this.options = options;
    this.shutdownWaitMs =
      options.nativeDrainEnvelopeMs ??
      (options.platform === 'darwin' ? MAC_SHUTDOWN_WAIT_MS : WINDOWS_SHUTDOWN_WAIT_MS);
    this.predecessorDrainWaitMs = options.predecessorDrainEnvelopeMs ?? PREDECESSOR_DRAIN_WAIT_MS;
    this.spawnHelper = options.spawnHelper ?? defaultSpawnHelper;
    this.rpcChannel = new HelperRpcChannel({
      createError: (code, message, rpcCode = null) => new HelperClientError(code, message, rpcCode),
      onFault: (session, reason, pendingError) =>
        this.handleRpcFault(session, reason, pendingError),
      onNotification: (session, notification) => this.publishNotification(session, notification),
      onPingResult: (session, result) => {
        if (
          this.rpcSession === session &&
          this.ownerAssociation !== null &&
          !this.ownerAssociationMatches(result.keyboardOwner)
        ) {
          this.pendingAuthoritativeActivation.length = 0;
          this.pendingActivationPolicy = 'replay-current-gesture';
          this.rpcChannel.resetOwnerActivationStream(session);
          this.sessionAuthoritative = false;
          if (this.healthRefresh?.session !== session && this.launching === null) {
            this.setReadiness({
              status: 'starting',
              reason: null,
              helperVersion: this.readiness.helperVersion,
              permissions: this.readiness.permissions,
            });
            queueMicrotask(() => {
              void this.reconcileLocalOwnerChange(
                session,
                result.keyboardOwner,
                result.hookStatus,
                this.readiness.permissions,
              ).catch(() => {
                if (this.healthSessionIsActive(session)) {
                  this.terminateCurrent('owner-degraded', true);
                }
              });
            });
          }
        }
      },
    });
    this.activation = new HelperActivationReconciler({
      getSession: () => this.rpcSession,
      isSessionAvailable: (session) =>
        this.sessionAuthoritative &&
        !this.maintenancePreparing &&
        !this.maintenancePrepared &&
        !this.terminating &&
        this.desiredRunning &&
        this.stopOperation === null &&
        this.rpcChannel.isCurrent(session),
      isSessionCurrent: (session) =>
        !this.maintenancePreparing &&
        !this.maintenancePrepared &&
        !this.terminating &&
        this.desiredRunning &&
        this.stopOperation === null &&
        this.rpcChannel.isCurrent(session),
      request: (session, params, timeoutMs, timeoutReason) =>
        this.rpcChannel.request(session, 'activation.configure', params, {
          timeoutMs,
          timeoutReason,
          allowDraining: false,
          supervision: true,
        }),
      createNotRunningError: (message) => new HelperClientError('not-running', message),
      createProtocolError: (message) => new HelperClientError('rpc-error', message),
      reportProtocolFault: (session, error) =>
        this.handleRpcFault(session, 'malformed-response', error),
      isNotRunningError: (error) =>
        error instanceof HelperClientError && error.code === 'not-running',
    });
  }

  readonly refreshHealthCoalesced = refreshHealthCoalesced;
  readonly refreshHealth = refreshHealth;
  readonly healthSessionIsActive = healthSessionIsActive;
  readonly reconcileLocalOwnerChange = reconcileLocalOwnerChange;
  readonly ordinaryRequestsAvailable = ordinaryRequestsAvailable;
  readonly ownerAssociationMatches = ownerAssociationMatches;
  readonly launch = launch;
  readonly validateHandshake = validateHandshake;
  readonly canLaunch = canLaunch;
  readonly isActiveChild = isActiveChild;
  readonly prepareOwnerMaintenance = prepareOwnerMaintenance;
  readonly startForIntent = startForIntent;
  readonly stopCurrentProcess = stopCurrentProcess;
  readonly intentIsCurrent = intentIsCurrent;
  readonly assertResetAllowed = assertResetAllowed;
  readonly requestBoundedShutdown = requestBoundedShutdown;
  readonly performStop = performStop;
  readonly attachProcess = attachProcess;
  readonly waitForStartupStderr = waitForStartupStderr;
  readonly parseDiagnosticLine = parseDiagnosticLine;
  readonly commitAndAcknowledgeOwnerDiagnostic = commitAndAcknowledgeOwnerDiagnostic;
  readonly publishProcessLifecycle = publishProcessLifecycle;
  readonly publishRuntimeObservability = publishRuntimeObservability;
  readonly handleRpcFault = handleRpcFault;
  readonly handleClose = handleClose;
  readonly terminateCurrent = terminateCurrent;
  readonly recordFailure = recordFailure;
  readonly openCrashLoop = openCrashLoop;
  readonly scheduleRestart = scheduleRestart;
  readonly startHeartbeat = startHeartbeat;
  readonly clearRestart = clearRestart;
  readonly clearHeartbeat = clearHeartbeat;
  readonly publishNotification = publishNotification;
  readonly publishAuthoritativeNotification = publishAuthoritativeNotification;
  readonly queueAuthoritativeActivation = queueAuthoritativeActivation;
  readonly flushAuthoritativeActivation = flushAuthoritativeActivation;
  readonly setReadiness = setReadiness;
}
