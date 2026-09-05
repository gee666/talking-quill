import { type HelperNotification } from '../../shared/helper/protocol';
import { HelperReadinessSchema, type HelperReadiness } from '../../shared/schemas/helper-readiness';
import { type HelperRpcSession } from './helper-rpc-channel';
import { saturatingSafeIncrement } from './helper-client-diagnostics';
import { type HelperClientRuntime } from './helper-client-runtime';

export function publishNotification(
  this: HelperClientRuntime,
  session: HelperRpcSession,
  notification: HelperNotification,
): void {
  if (this.rpcSession !== session || this.ownerAssociation === null) return;
  if (!this.sessionAuthoritative) {
    if (notification.method === 'activation.event') {
      this.queueAuthoritativeActivation(notification);
    }
    return;
  }
  this.publishAuthoritativeNotification(notification);
}

export function publishAuthoritativeNotification(
  this: HelperClientRuntime,
  notification: HelperNotification,
): void {
  if (
    (notification.method === 'activation.event' && this.captureDisabledForSession) ||
    (notification.method === 'session.key' && this.sessionKeyCaptureAvailable === false)
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
    this.electronRegisteredObservations = saturatingSafeIncrement(
      this.electronRegisteredObservations,
    );
  }
  for (const listener of this.notificationListeners) {
    try {
      listener(notification);
    } catch {
      // Native protocol health must not depend on a consumer callback.
    }
  }
}

export function queueAuthoritativeActivation(
  this: HelperClientRuntime,
  notification: Extract<HelperNotification, { method: 'activation.event' }>,
): void {
  if (this.pendingActivationPolicy === 'drop') return;
  if (notification.params.phase !== 'up') {
    // The latest valid start supersedes an older unbalanced candidate. This keeps a bounded
    // queue while allowing down(1), down(2), up(2) to replay the current gesture exactly.
    this.pendingAuthoritativeActivation = [notification];
    return;
  }
  const first = this.pendingAuthoritativeActivation[0];
  if (
    first?.params.phase === 'down' &&
    notification.params.activationGeneration === first.params.activationGeneration &&
    notification.params.targetToken === first.params.targetToken
  ) {
    this.pendingAuthoritativeActivation = [first, notification];
  }
}

export function flushAuthoritativeActivation(this: HelperClientRuntime): void {
  if (!this.sessionAuthoritative || this.readiness.status !== 'ready') return;
  const pending = this.pendingAuthoritativeActivation.splice(0);
  this.pendingActivationPolicy = 'drop';
  for (const notification of pending) {
    this.publishAuthoritativeNotification(notification);
  }
}

export function setReadiness(this: HelperClientRuntime, readiness: HelperReadiness): void {
  const validated = HelperReadinessSchema.parse(readiness);
  if (JSON.stringify(validated) === JSON.stringify(this.readiness)) return;
  this.readiness = Object.freeze(validated);
  for (const listener of this.readinessListeners) {
    try {
      listener(this.readiness);
    } catch {
      // Readiness observers are isolated from helper supervision.
    }
  }
}
