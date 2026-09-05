import {
  waitForClose,
  waitForOutcomeWithin,
  waitForValueWithin,
  waitForCloseWithin,
} from './helper-process';
import { ownerReasonFromRpcCode } from './helper-readiness';
import { HelperClientError } from './helper-client-error';
import { performance } from 'node:perf_hooks';
import {
  type HelperPrepareMaintenanceParams,
  type HelperResult,
} from '../../shared/helper/protocol';
import { type HelperRpcSession } from './helper-rpc-channel';
import { SHUTDOWN_EXIT_MARGIN_MS } from './helper-client-options';
import { type HelperClientRuntime } from './helper-client-runtime';

export async function prepareOwnerMaintenance(
  this: HelperClientRuntime,
  params: HelperPrepareMaintenanceParams,
  timeoutMs: number,
  signal?: AbortSignal,
) {
  if (!Number.isFinite(timeoutMs) || timeoutMs <= 0) {
    throw new HelperClientError('request-timeout', 'Native helper maintenance deadline is invalid');
  }
  const session = this.rpcSession;
  if (!this.ordinaryRequestsAvailable() || session === null) {
    throw new HelperClientError('not-running', 'Native helper is terminating');
  }
  const deadlineAt = performance.now() + timeoutMs;
  this.maintenancePreparing = true;
  this.clearHeartbeat();
  const dispatchState = { dispatched: false };
  let result: HelperResult<'owner.prepare_maintenance'>;
  try {
    const remaining = deadlineAt - performance.now();
    if (remaining <= 0) {
      throw new HelperClientError('request-timeout', 'Native helper maintenance deadline expired');
    }
    result = await this.rpcChannel.request(session, 'owner.prepare_maintenance', params, {
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
      this.maintenancePreparing = false;
      const child = this.child;
      if (child !== null && this.ordinaryRequestsAvailable()) {
        this.startHeartbeat(child, session);
      }
    } else if (this.rpcSession === session) {
      const reason =
        error instanceof HelperClientError
          ? (ownerReasonFromRpcCode(error.rpcCode) ?? 'owner-indeterminate')
          : 'owner-indeterminate';
      this.terminateCurrent(reason, false, error instanceof Error ? error : undefined);
    }
    throw error;
  }
  if (this.rpcSession !== session || !this.rpcChannel.isCurrent(session)) {
    throw new HelperClientError('not-running', 'Native helper maintenance changed process');
  }
  this.maintenancePreparing = false;
  this.maintenancePrepared = true;
  this.sessionAuthoritative = false;
  this.captureDisabledForSession = true;
  this.sessionKeyCaptureAvailable = false;
  this.activation.processUnavailable(session);
  this.setReadiness({
    status: 'unavailable',
    reason: 'owner-maintenance',
    helperVersion: this.readiness.helperVersion,
    permissions: this.readiness.permissions,
  });
  this.rpcChannel.beginDraining(session);
  const stopping = this.stopCurrentProcess();
  const stopped = await waitForOutcomeWithin(stopping, Math.max(0, deadlineAt - performance.now()));
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

export async function startForIntent(this: HelperClientRuntime, revision: number): Promise<void> {
  const stopping = this.stopOperation;
  if (stopping !== null) await stopping;
  if (!this.intentIsCurrent(revision)) return;
  if (this.launching !== null) {
    await this.launching;
    return;
  }
  if (!this.canLaunch()) return;
  if (this.crashLoopOpen) this.halfOpenProbe = true;
  this.launching = this.launch().finally(() => {
    this.launching = null;
  });
  await this.launching;
}

export async function stopCurrentProcess(
  this: HelperClientRuntime,
  requireNeutral = false,
): Promise<void> {
  if (this.stopOperation !== null) return this.stopOperation;
  this.stopTerminalFault = null;
  const session = this.rpcSession;
  if (session !== null) this.rpcChannel.beginDraining(session);
  const operation = Promise.resolve().then(() => this.performStop(requireNeutral));
  this.stopOperation = operation;
  try {
    await operation;
  } finally {
    if (this.stopOperation === operation) this.stopOperation = null;
  }
}

export function intentIsCurrent(this: HelperClientRuntime, revision: number): boolean {
  return this.desiredRunning && this.runIntentRevision === revision;
}

export function assertResetAllowed(
  this: HelperClientRuntime,
  revision: number,
  signal?: AbortSignal,
): void {
  if (signal?.aborted === true) {
    throw new DOMException('Native helper capture reset cancelled', 'AbortError');
  }
  if (!this.intentIsCurrent(revision)) {
    throw new HelperClientError('not-running', 'Native helper capture reset was superseded');
  }
}

export async function requestBoundedShutdown(
  this: HelperClientRuntime,
  session: HelperRpcSession,
): Promise<{
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
  const completion = this.rpcChannel.request(
    session,
    'shutdown',
    {},
    {
      timeoutMs: this.shutdownWaitMs,
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
  const dispatch = await waitForValueWithin(dispatched, this.predecessorDrainWaitMs);
  if (!dispatch.completed) {
    abort.abort();
    await outcome;
    return {
      dispatchAt: null,
      succeeded: false,
      ownerDisposition: null,
      error: new HelperClientError('request-timeout', 'Native helper predecessor drain timed out'),
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

export async function performStop(
  this: HelperClientRuntime,
  requireNeutral: boolean,
): Promise<void> {
  this.clearRestart();
  this.clearHeartbeat();
  const child = this.child;
  if (child !== null) {
    this.plannedExit = { reason: 'shutdown', restart: false, lifecycle: true };
    this.terminating = true;
    const close = waitForClose(child);
    const session = this.rpcSession;
    let shutdownError: unknown = null;
    let shutdownSucceeded = false;
    let shutdownDispatchedAt: number | null = null;
    let ownerDisposition: 'neutral' | 'draining' | null = null;
    try {
      if (session !== null && this.rpcChannel.isCurrent(session)) {
        const shutdown = await this.requestBoundedShutdown(session);
        shutdownDispatchedAt = shutdown.dispatchAt;
        shutdownSucceeded = shutdown.succeeded;
        ownerDisposition = shutdown.ownerDisposition;
        shutdownError = shutdown.error;
      }
      const remaining =
        shutdownDispatchedAt === null
          ? 0
          : Math.max(0, this.shutdownWaitMs - (performance.now() - shutdownDispatchedAt));
      let closed = await waitForCloseWithin(close.promise, remaining);
      if (!closed && shutdownError === null) {
        closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
      }
      if (!closed && this.child === child) {
        if (session !== null && this.rpcSession === session) {
          this.rpcChannel.close(
            session,
            new HelperClientError('transport-error', 'Native helper shutdown timed out'),
          );
          this.activation.processUnavailable(session);
        }
        child.kill();
        closed = await waitForCloseWithin(close.promise, SHUTDOWN_EXIT_MARGIN_MS);
      }
      if (!closed && this.child === child) {
        throw new HelperClientError('transport-error', 'Native helper exit could not be confirmed');
      }
      if (
        this.stopTerminalFault !== null ||
        !shutdownSucceeded ||
        child.exitCode !== 0 ||
        (requireNeutral && ownerDisposition !== 'neutral')
      ) {
        const stopError =
          this.stopTerminalFault ??
          (shutdownError instanceof Error
            ? shutdownError
            : new HelperClientError(
                'transport-error',
                'Native helper did not complete clean shutdown',
              ));
        throw stopError;
      }
    } finally {
      close.cancel();
    }
  }
  if (!this.desiredRunning) {
    this.setReadiness({
      status: 'stopped',
      reason: 'shutdown',
      helperVersion: this.readiness.helperVersion,
      permissions: this.readiness.permissions,
    });
  }
}
