import type { MicrophoneTestState } from '../../shared/schemas/audio';
import { CaptureClientError } from './capture-client-error';
import type { RecordingContext } from './recording-context';
import { publishDeviceSnapshot } from './recording-snapshot';

// Evidence publication must not import refresh, rebind, or capture orchestration.
export function setWelcomeEvidenceInvalidator(
  context: RecordingContext,
  listener: () => void,
): void {
  context.onMicrophoneUnavailable = listener;
}

export function setWelcomeEvidenceValidationListener(
  context: RecordingContext,
  listener: (known: boolean) => void,
): void {
  context.onMicrophoneValidationChanged = listener;
}

export function microphoneReadyForWelcome(context: RecordingContext): boolean {
  return context.permission.getStatus() === 'granted' && context.welcomeMicrophoneBindingKnown;
}

export function setWelcomeMicrophoneBindingKnown(context: RecordingContext, known: boolean): void {
  if (context.welcomeMicrophoneBindingKnown === known) return;
  context.welcomeMicrophoneBindingKnown = known;
  try {
    context.onMicrophoneValidationChanged?.(known);
  } catch {
    // Recording ownership remains authoritative if Welcome validation fails.
  }
  publishDeviceSnapshot(context);
}

export function invalidateDefaultEvidence(context: RecordingContext): void {
  invalidateActiveTestEvidence(context);
  notifyMicrophoneUnavailable(context);
}

export function invalidateEvidenceForStartupFailure(
  context: RecordingContext,
  error: unknown,
): void {
  if (
    error instanceof CaptureClientError &&
    (error.code === 'no-device' || error.code === 'device-unavailable')
  ) {
    setWelcomeMicrophoneBindingKnown(context, false);
    notifyMicrophoneUnavailable(context);
  }
}

export function setFailureState(
  context: RecordingContext,
  error: unknown,
  captureId: string,
): void {
  const code = error instanceof CaptureClientError ? error.code : 'capture-failed';
  if (code === 'permission-denied') {
    if (context.permission.takePolicyDenial(captureId)) {
      console.error('Talking Quill microphone request rejected by application policy', {
        code: 'MICROPHONE_POLICY_DENIED',
      });
      setState(context, {
        status: 'unavailable',
        permission: context.permission.getStatus(),
        reason: 'permission-unavailable',
      });
      return;
    }
    const permission = context.permission.getStatus();
    if (permission === 'denied' || permission === 'restricted') {
      setState(context, { status: 'blocked', permission, reason: 'microphone-permission' });
    } else {
      setState(context, {
        status: 'unavailable',
        permission,
        reason: 'permission-unavailable',
      });
    }
    return;
  }
  const reason =
    code === 'no-device'
      ? 'no-device'
      : code === 'device-unavailable'
        ? 'device-unavailable'
        : code === 'unsupported-audio-format'
          ? 'unsupported-audio-format'
          : 'capture-unavailable';
  setState(context, {
    status: 'unavailable',
    permission: context.permission.getStatus(),
    reason,
  });
}

export function invalidateActiveTestEvidence(context: RecordingContext): void {
  if (context.activeCaptureKind !== 'test' || context.state.status !== 'active') return;
  context.lastLevelEventAt = 0;
  context.testObservedRms = 0;
  context.testSampleCount = 0;
  try {
    context.events.send('recording:test-level', {
      captureId: context.state.captureId,
      rms: 0,
    });
  } catch {
    // The test metadata remains authoritative if its renderer disappeared.
  }
}

export function notifyMicrophoneUnavailable(context: RecordingContext): void {
  try {
    context.onMicrophoneUnavailable?.();
  } catch {
    // Evidence invalidation is ancillary to releasing the failed capture.
  }
}

export function setState(context: RecordingContext, state: MicrophoneTestState): void {
  context.state = state;
  try {
    context.events.send('recording:test-state-changed', state);
  } catch {
    // State remains authoritative if its renderer disappeared during publication.
  }
}
