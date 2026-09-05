import { type HelperParams, type HelperResult } from '../../shared/helper/protocol';
import { shortcutsEqual } from '../../shared/schemas/shortcut';
import {
  type HelperRpcRuntime,
  type ActivationNotification,
  type PairedActivationParams,
} from './helper-rpc-runtime';

const MAX_DEFERRED_ACTIVATION_EVENTS = 16;

export function deferActivationEvent(
  this: HelperRpcRuntime,
  notification: ActivationNotification,
): boolean {
  if (this.dispatchedId === null) return false;
  const pending = this.pending.get(this.dispatchedId);
  if (pending?.method !== 'activation.configure') return false;
  const deferred = this.deferredActivationEvents;
  const startsReplacementStream = deferred === null && notification.params.phase !== 'up';
  if (!startsReplacementStream && deferred?.requestId !== this.dispatchedId) return false;
  const events = deferred?.notifications ?? [];
  if (events.length >= MAX_DEFERRED_ACTIVATION_EVENTS) {
    throw new Error('Deferred helper activation capacity exceeded');
  }
  if (deferred === null) {
    this.deferredActivationEvents = {
      requestId: this.dispatchedId,
      notifications: [notification],
    };
  } else {
    events.push(notification);
  }
  return true;
}

export function validateActivationEvent(
  this: HelperRpcRuntime,
  notification: ActivationNotification,
): void {
  const params = notification.params;
  if (params.phase === 'up') {
    const active = this.activeActivation;
    if (active === null || !activationParamsMatch(active, params)) {
      throw new Error('Unpaired helper activation event');
    }
    this.activeActivation = null;
    return;
  }

  if (
    params.activationGeneration <= this.lastActivationGeneration ||
    this.activeActivation !== null
  ) {
    throw new Error('Non-monotonic helper activation generation');
  }
  this.lastActivationGeneration = params.activationGeneration;
  if (params.phase === 'down') this.activeActivation = params;
}

export function activationAcknowledgementMatches(
  requested: HelperParams<'activation.configure'>,
  effective: HelperResult<'activation.configure'>,
): boolean {
  return (
    effective.enabled === requested.enabled &&
    effective.bindings.length === requested.bindings.length &&
    effective.bindings.every((binding, index) => {
      const candidate = requested.bindings[index];
      return (
        binding.profileId === candidate?.profileId &&
        shortcutsEqual(binding.shortcut, candidate.shortcut)
      );
    })
  );
}

function activationParamsMatch(
  left: PairedActivationParams,
  right: PairedActivationParams,
): boolean {
  return (
    left.activationGeneration === right.activationGeneration &&
    left.targetToken === right.targetToken &&
    left.profileId === right.profileId &&
    shortcutsEqual(left.shortcut, right.shortcut)
  );
}
