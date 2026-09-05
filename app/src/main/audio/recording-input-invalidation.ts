import type { RecordingContext } from './recording-context';
import { refreshAfterInputInvalidation } from './recording-devices';
import {
  invalidateActiveTestEvidence,
  invalidateDefaultEvidence,
  notifyMicrophoneUnavailable,
  setWelcomeMicrophoneBindingKnown,
} from './recording-evidence';
import { queueDefaultRebind } from './recording-rebind';

export function invalidateInputDevices(context: RecordingContext): void {
  if (context.disposed) return;
  if (context.settings.get().recording.preferredMicrophoneId !== null) {
    validateExplicitBindingAfterInvalidation(context);
    if (context.activeCaptureId !== null && context.activePreferredUnavailable) {
      queueDefaultRebind(context, context.activeBindingGeneration);
    }
    return;
  }
  refreshAfterInputInvalidation(context);
  invalidateDefaultEvidence(context);
  if (context.activeCaptureId !== null) {
    queueDefaultRebind(context, context.activeBindingGeneration);
  }
}

export function validateExplicitBindingAfterInvalidation(context: RecordingContext): void {
  const preferred = context.settings.get().recording.preferredMicrophoneId;
  if (preferred === null) return;
  const validationGeneration = ++context.explicitValidationGeneration;
  setWelcomeMicrophoneBindingKnown(context, false);
  const refresh = refreshAfterInputInvalidation(context);
  void refresh.promise.then(() => {
    if (
      context.disposed ||
      validationGeneration !== context.explicitValidationGeneration ||
      context.settings.get().recording.preferredMicrophoneId !== preferred
    ) {
      return;
    }
    const authoritative = context.lastAuthorizedDeviceRefreshGeneration >= refresh.generation;
    if (authoritative) {
      if (
        context.devices.some((device) => device.deviceId === preferred) &&
        !(context.activePreferredMicrophoneId === preferred && context.activePreferredUnavailable)
      ) {
        if (context.activePreferredMicrophoneId === preferred) {
          context.activeExplicitDeviceAbsent = false;
        }
        setWelcomeMicrophoneBindingKnown(context, true);
      }
      return;
    }
    if (context.activePreferredMicrophoneId === preferred) {
      context.activeExplicitDeviceAbsent = true;
      invalidateActiveTestEvidence(context);
    }
    notifyMicrophoneUnavailable(context);
  });
}
