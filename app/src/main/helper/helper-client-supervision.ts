import { waitForClose, waitForCloseWithin } from './helper-process';
import {
  classifyLaunchError,
  readinessReasonFromNativeLaunchFailure,
  isOwnerTransition,
  shouldRestartAfterFailure,
} from './helper-readiness';
import { HelperClientError } from './helper-client-error';
import { type ChildProcessWithoutNullStreams } from './helper-process';
import { performance } from 'node:perf_hooks';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { type HelperRpcSession } from './helper-rpc-channel';
import {
  HEARTBEAT_INTERVAL_MS,
  SHUTDOWN_EXIT_MARGIN_MS,
  FAILURE_WINDOW_MS,
  FAILURE_LIMIT,
  RESTART_DELAYS_MS,
} from './helper-client-options';
import { type HelperClientRuntime } from './helper-client-runtime';

export function handleRpcFault(
  this: HelperClientRuntime,
  session: HelperRpcSession,
  reason: HelperReadinessReason,
  pendingError?: Error,
): void {
  if (this.rpcSession !== session) return;
  // During startup the rejected request is only one half of the process
  // outcome. Coordinate it with bounded stderr drainage before selecting
  // retry policy and publishing readiness. The RPC channel intentionally
  // leaves pending requests open until supervision closes it.
  if (this.launching !== null && this.stopOperation === null) {
    const generation = this.generation;
    void (async () => {
      await this.waitForStartupStderr(generation);
      if (this.rpcSession !== session || this.generation !== generation) return;
      const startupReason =
        readinessReasonFromNativeLaunchFailure(this.nativeLaunchFailure) ?? reason;
      this.terminateCurrent(startupReason, shouldRestartAfterFailure(startupReason), pendingError);
    })();
    return;
  }
  const effectiveReason =
    readinessReasonFromNativeLaunchFailure(this.nativeLaunchFailure) ?? reason;
  if (this.stopOperation !== null) {
    if (reason !== 'malformed-response') return;
    this.stopTerminalFault ??=
      pendingError ??
      new HelperClientError('transport-error', 'Native helper returned malformed output');
    this.setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: this.readiness.helperVersion,
      permissions: this.readiness.permissions,
    });
    // The channel released malformed predecessor authority without pumping.
    // Drain policy is authoritative now, so only reserved shutdown may run.
    this.rpcChannel.beginDraining(session);
    return;
  }
  this.terminateCurrent(effectiveReason, shouldRestartAfterFailure(effectiveReason), pendingError);
}

export function handleClose(
  this: HelperClientRuntime,
  child: ChildProcessWithoutNullStreams,
  session: HelperRpcSession,
  generation: number,
): void {
  if (this.child !== child || this.rpcSession !== session || this.generation !== generation) {
    return;
  }
  this.rpcChannel.close(session, new HelperClientError('transport-error', 'Native helper stopped'));
  this.activation.processUnavailable(session);
  this.child = null;
  this.rpcSession = null;
  this.sessionAuthoritative = false;
  this.maintenancePreparing = false;
  this.maintenancePrepared = false;
  this.captureDisabledForSession = false;
  this.captureBuildDisabledForSession = false;
  this.runtimeRollbackForSession = false;
  this.sessionKeyCaptureAvailable = null;
  this.ownerAssociation = null;
  this.pendingAuthoritativeActivation.length = 0;
  this.pendingActivationPolicy = 'drop';
  this.terminating = false;
  this.clearHeartbeat();
  const planned = this.plannedExit;
  this.plannedExit = null;
  if (!this.desiredRunning || this.stopOperation !== null) return;
  const nativeReason = readinessReasonFromNativeLaunchFailure(this.nativeLaunchFailure);
  const reason = nativeReason ?? planned?.reason ?? 'unexpected-exit';
  this.recordFailure(
    reason,
    nativeReason === null ? (planned?.restart ?? true) : shouldRestartAfterFailure(nativeReason),
  );
}

export function terminateCurrent(
  this: HelperClientRuntime,
  reason: HelperReadinessReason,
  restart: boolean,
  pendingError: Error = new HelperClientError(
    'transport-error',
    'Native helper transport is terminating',
  ),
): void {
  const child = this.child;
  const session = this.rpcSession;
  if (child === null || session === null || this.terminating) return;
  this.terminating = true;
  this.plannedExit ??= { reason, restart, lifecycle: false };
  this.pendingAuthoritativeActivation.length = 0;
  this.pendingActivationPolicy = 'drop';
  this.activation.processUnavailable(session);
  this.clearHeartbeat();
  this.setReadiness({
    status:
      !this.desiredRunning && reason === 'shutdown'
        ? 'stopped'
        : reason === 'owner-incompatible'
          ? 'incompatible'
          : 'unavailable',
    reason,
    helperVersion: this.readiness.helperVersion,
    permissions: this.readiness.permissions,
  });
  this.rpcChannel.beginDraining(session);
  const close = waitForClose(child);
  void (async () => {
    try {
      const shutdown = await this.requestBoundedShutdown(session);
      const remaining =
        shutdown.dispatchAt === null
          ? 0
          : Math.max(0, this.shutdownWaitMs - (performance.now() - shutdown.dispatchAt));
      let closed = await waitForCloseWithin(close.promise, remaining);
      if (!closed && this.child === child) {
        if (this.rpcSession === session) this.rpcChannel.close(session, pendingError);
        child.kill();
        closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
      }
      if (!closed && this.child === child) {
        this.setReadiness({
          status: 'unavailable',
          reason: 'hook-fault',
          helperVersion: this.readiness.helperVersion,
          permissions: this.readiness.permissions,
        });
      }
    } finally {
      close.cancel();
    }
  })();
}

export function recordFailure(
  this: HelperClientRuntime,
  reason: HelperReadinessReason,
  restart = true,
): void {
  if (isOwnerTransition(reason)) {
    // A predecessor may retain the owner singleton while it drains held key
    // state. Keep this outside crash accounting so normal handoff cannot
    // open Electron's two-minute crash-loop circuit.
    this.failureTimes = [];
    this.crashLoopOpen = false;
    this.halfOpenProbe = false;
    this.setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: this.readiness.helperVersion,
      permissions: this.readiness.permissions,
    });
    if (restart) this.scheduleRestart(RESTART_DELAYS_MS[0]);
    else this.clearRestart();
    return;
  }
  if (!restart) {
    this.failureTimes = [];
    this.crashLoopOpen = false;
    this.halfOpenProbe = false;
    this.clearRestart();
    this.setReadiness({
      status:
        reason === 'protocol-mismatch' || reason === 'owner-incompatible'
          ? 'incompatible'
          : 'unavailable',
      reason,
      helperVersion: this.readiness.helperVersion,
      permissions: this.readiness.permissions,
    });
    return;
  }

  const now = Date.now();
  if (this.halfOpenProbe) {
    this.openCrashLoop();
    return;
  }
  this.failureTimes = this.failureTimes.filter((time) => now - time < FAILURE_WINDOW_MS);
  this.failureTimes.push(now);
  if (this.failureTimes.length >= FAILURE_LIMIT) {
    this.openCrashLoop();
    return;
  }

  this.setReadiness({
    status: 'unavailable',
    reason,
    helperVersion: this.readiness.helperVersion,
    permissions: this.readiness.permissions,
  });
  const delayIndex = Math.min(this.failureTimes.length - 1, RESTART_DELAYS_MS.length - 1);
  this.scheduleRestart(RESTART_DELAYS_MS[delayIndex] ?? RESTART_DELAYS_MS[0]);
}

export function openCrashLoop(this: HelperClientRuntime): void {
  this.failureTimes = [];
  this.crashLoopOpen = true;
  this.halfOpenProbe = false;
  this.setReadiness({
    status: 'unavailable',
    reason: 'crash-loop',
    helperVersion: this.readiness.helperVersion,
    permissions: this.readiness.permissions,
  });
  this.scheduleRestart(FAILURE_WINDOW_MS);
}

export function scheduleRestart(this: HelperClientRuntime, restartAfter: number): void {
  const revision = this.runIntentRevision;
  this.clearRestart();
  this.restartTimer = setTimeout(() => {
    this.restartTimer = null;
    if (this.intentIsCurrent(revision) && this.child === null) {
      void this.startForIntent(revision);
    }
  }, restartAfter);
  this.restartTimer.unref();
}

export function startHeartbeat(
  this: HelperClientRuntime,
  child: ChildProcessWithoutNullStreams,
  session: HelperRpcSession,
): void {
  this.clearHeartbeat();
  this.heartbeatTimer = setInterval(() => {
    if (this.child !== child || this.rpcSession !== session) return;
    void this.refreshHealthCoalesced(session).catch((error: unknown) => {
      if (error instanceof HelperClientError && error.code === 'request-capacity') return;
      if (this.rpcSession === session && this.desiredRunning && this.stopOperation === null) {
        const reason = classifyLaunchError(error);
        if (isOwnerTransition(reason)) {
          this.pendingAuthoritativeActivation.length = 0;
          this.pendingActivationPolicy = 'drop';
          this.setReadiness({
            status: 'unavailable',
            reason,
            helperVersion: this.readiness.helperVersion,
            permissions: this.readiness.permissions,
          });
          return;
        }
        this.terminateCurrent(
          error instanceof HelperClientError && error.code === 'request-timeout'
            ? 'request-timeout'
            : reason,
          shouldRestartAfterFailure(reason),
        );
      }
    });
  }, HEARTBEAT_INTERVAL_MS);
  this.heartbeatTimer.unref();
}

export function clearRestart(this: HelperClientRuntime): void {
  if (this.restartTimer !== null) clearTimeout(this.restartTimer);
  this.restartTimer = null;
}

export function clearHeartbeat(this: HelperClientRuntime): void {
  if (this.heartbeatTimer !== null) clearInterval(this.heartbeatTimer);
  this.heartbeatTimer = null;
}
