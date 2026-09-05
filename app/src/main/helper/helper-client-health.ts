import { readinessFromOwner, hookTransportReady, permissionsAreGranted } from './helper-readiness';
import { HelperClientError } from './helper-client-error';
import {
  type HelperInitializeResult,
  type HelperKeyboardOwnerSnapshot,
  type HelperPermissions,
} from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { type HelperRpcSession } from './helper-rpc-channel';
import { REQUEST_TIMEOUT_MS } from './helper-client-options';
import { type HelperClientRuntime } from './helper-client-runtime';

export function refreshHealthCoalesced(
  this: HelperClientRuntime,
  session: HelperRpcSession | null,
): Promise<HelperPermissions> {
  if (this.healthRefresh?.session === session) return this.healthRefresh.operation;
  const operation = this.refreshHealth(session).finally(() => {
    if (this.healthRefresh?.operation === operation) this.healthRefresh = null;
  });
  this.healthRefresh = { session, operation };
  return operation;
}

export async function refreshHealth(
  this: HelperClientRuntime,
  session: HelperRpcSession | null,
): Promise<HelperPermissions> {
  if (session === null || !this.desiredRunning) {
    throw new HelperClientError('not-running', 'Native helper is terminating');
  }
  // Fence ordinary work and notifications behind this authenticated owner snapshot.
  // Requests already dispatched remain ordered before the health check. One gesture from the
  // same authenticated owner may be replayed after a successful disabled-first reconciliation.
  this.sessionAuthoritative = false;
  this.pendingActivationPolicy = 'replay-current-gesture';
  const [permissions, health] = await Promise.all([
    this.rpcChannel.request(
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
    this.rpcChannel.request(
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
  if (!this.healthSessionIsActive(session)) return permissions;
  if (!this.ownerAssociationMatches(health.keyboardOwner)) {
    await this.reconcileLocalOwnerChange(
      session,
      health.keyboardOwner,
      health.hookStatus,
      permissions,
    );
    return permissions;
  }
  this.missingOwnerHealthChecks = 0;
  this.sessionAuthoritative = true;
  const readiness = readinessFromOwner(
    this.readiness.helperVersion,
    health.hookStatus,
    permissions,
    health.keyboardOwner,
    this.captureDisabledForSession,
    this.runtimeRollbackForSession,
  );
  this.activation.setBlockedByHealth(readiness.status !== 'ready');
  const ownerUnavailable = readiness.reason?.startsWith('owner-') === true;
  if (this.captureDisabledForSession || ownerUnavailable) {
    this.pendingAuthoritativeActivation.length = 0;
    this.pendingActivationPolicy = 'drop';
    this.sessionAuthoritative = false;
    this.setReadiness(readiness);
    return permissions;
  }
  try {
    await this.activation.reconcileSession(session, false);
  } catch (error: unknown) {
    if (!this.healthSessionIsActive(session)) return permissions;
    this.terminateCurrent(readiness.reason ?? 'hook-fault', true);
    throw error;
  }
  if (!this.healthSessionIsActive(session)) return permissions;
  this.setReadiness(readiness);
  this.flushAuthoritativeActivation();
  if (
    readiness.reason === 'hook-fault' &&
    readiness.status === 'unavailable' &&
    permissionsAreGranted(permissions) &&
    !hookTransportReady(health.hookStatus)
  ) {
    // A macOS event tap created without permission cannot become live in place.
    // Recycle only after activation is confirmed disabled so the replacement
    // can recreate the hook and restore the retained desired configuration.
    this.terminateCurrent('hook-fault', true);
  }
  return permissions;
}

export function healthSessionIsActive(
  this: HelperClientRuntime,
  session: HelperRpcSession,
): boolean {
  return (
    this.rpcSession === session &&
    this.desiredRunning &&
    this.stopOperation === null &&
    this.rpcChannel.isCurrent(session)
  );
}

export async function reconcileLocalOwnerChange(
  this: HelperClientRuntime,
  session: HelperRpcSession,
  owner: HelperKeyboardOwnerSnapshot,
  hookStatus: HelperInitializeResult['hookStatus'],
  permissions: HelperPermissions,
): Promise<void> {
  const previous = this.ownerAssociation;
  if (!owner.authenticated || owner.leaseEpoch === null) {
    const reason: HelperReadinessReason =
      owner.state === 'unavailable' ? 'owner-missing' : 'owner-auth-failed';
    this.pendingAuthoritativeActivation.length = 0;
    this.pendingActivationPolicy = 'drop';
    this.sessionAuthoritative = false;
    this.activation.processUnavailable(session);
    this.setReadiness({
      status: 'unavailable',
      reason,
      helperVersion: this.readiness.helperVersion,
      permissions,
    });
    if (reason === 'owner-auth-failed') this.terminateCurrent(reason, false);
    else if (++this.missingOwnerHealthChecks >= 2) {
      // A cancelled native connector cannot recover inside the same gateway.
      // Give an ordinary reconnect one health interval, then restart the helper
      // and restore the desired bindings through disabled-first reconciliation.
      this.missingOwnerHealthChecks = 0;
      this.terminateCurrent(reason, true);
    }
    return;
  }
  this.missingOwnerHealthChecks = 0;
  if (previous === null || (previous.buildId !== '' && owner.buildId !== previous.buildId)) {
    this.pendingActivationPolicy = 'drop';
    this.terminateCurrent('owner-degraded', true);
    throw new HelperClientError('rpc-error', 'Native helper owner association changed');
  }

  this.sessionAuthoritative = false;
  this.pendingActivationPolicy = 'replay-current-gesture';
  this.setReadiness({
    status: 'starting',
    reason: null,
    helperVersion: this.readiness.helperVersion,
    permissions,
  });
  this.ownerAssociation = {
    instanceId: owner.instanceId,
    buildId: owner.buildId,
    leaseEpoch: owner.leaseEpoch,
  };
  this.captureDisabledForSession =
    this.captureBuildDisabledForSession || this.runtimeRollbackForSession;
  this.sessionKeyCaptureAvailable = !this.captureDisabledForSession;
  this.rpcChannel.resetOwnerActivationStream(session);
  this.activation.prepareFreshSession();
  const readiness = readinessFromOwner(
    this.readiness.helperVersion,
    hookStatus,
    permissions,
    owner,
    this.captureDisabledForSession,
    this.runtimeRollbackForSession,
  );
  this.activation.setBlockedByHealth(readiness.status !== 'ready');
  try {
    const capture = await this.rpcChannel.request(
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
    await this.activation.reconcileFreshHelper(
      session,
      REQUEST_TIMEOUT_MS,
      'request-timeout',
      () => {
        if (this.rpcSession === session && this.rpcChannel.isCurrent(session)) {
          this.sessionAuthoritative = true;
        }
      },
    );
  } catch (error: unknown) {
    if (this.healthSessionIsActive(session)) this.terminateCurrent('owner-degraded', true);
    throw error;
  }
  if (!this.healthSessionIsActive(session)) return;
  this.setReadiness(readiness);
  this.flushAuthoritativeActivation();
}

export function ordinaryRequestsAvailable(this: HelperClientRuntime): boolean {
  return (
    this.sessionAuthoritative &&
    !this.maintenancePreparing &&
    !this.maintenancePrepared &&
    !this.terminating &&
    this.desiredRunning &&
    this.stopOperation === null
  );
}

export function ownerAssociationMatches(
  this: HelperClientRuntime,
  owner: HelperKeyboardOwnerSnapshot,
): boolean {
  return (
    this.ownerAssociation !== null &&
    owner.instanceId === this.ownerAssociation.instanceId &&
    owner.buildId === this.ownerAssociation.buildId &&
    owner.leaseEpoch === this.ownerAssociation.leaseEpoch
  );
}
