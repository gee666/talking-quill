import type { HelperRuntimeObservability } from '../../shared/helper/protocol';
import type { ActivationTestState } from '../../shared/schemas/activation-test';

export function hasCompleteDedicatedTraversal(
  baseline: HelperRuntimeObservability['registeredInput'],
  current: HelperRuntimeObservability['registeredInput'],
): boolean {
  if (
    current.callbackChannelRejected !== baseline.callbackChannelRejected ||
    current.ownerRejected !== baseline.ownerRejected
  ) {
    return false;
  }
  return [
    'physicalCallbacks',
    'registeredCandidateCallbacks',
    'registeredMatchCallbacks',
    'registeredReleaseCallbacks',
    'callbackChannelAccepted',
    'adapterDequeued',
    'ownerAdmitted',
    'ownerFlushed',
    'gatewayReceived',
    'v10NotificationAccepted',
    'electronReceived',
  ].every(
    (field) =>
      current[field as keyof HelperRuntimeObservability['registeredInput']] >
      baseline[field as keyof HelperRuntimeObservability['registeredInput']],
  );
}

export function furthestObservationBoundary(
  baseline: HelperRuntimeObservability['registeredInput'],
  current: HelperRuntimeObservability['registeredInput'],
): ActivationTestState['furthestBoundary'] {
  const boundaries = [
    ['electron-received', 'electronReceived'],
    ['v10-notification', 'v10NotificationAccepted'],
    ['gateway-received', 'gatewayReceived'],
    ['owner-flushed', 'ownerFlushed'],
    ['owner-admitted', 'ownerAdmitted'],
    ['adapter-dequeued', 'adapterDequeued'],
    ['callback-channel', 'callbackChannelAccepted'],
    ['registered-release', 'registeredReleaseCallbacks'],
    ['registered-match', 'registeredMatchCallbacks'],
    ['registered-candidate', 'registeredCandidateCallbacks'],
    ['physical-callback', 'physicalCallbacks'],
    ['hook-callback', 'hcActionCallbacks'],
    ['pump-alive', 'pumpAlive'],
    ['hook-installed', 'hookInstalled'],
  ] as const;
  return boundaries.find(([, field]) => current[field] > baseline[field])?.[0] ?? null;
}
