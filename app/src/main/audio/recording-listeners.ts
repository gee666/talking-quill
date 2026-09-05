import type { RecordingContext } from './recording-context';
import { refreshAfterInputInvalidation } from './recording-devices';
import {
  invalidateDefaultEvidence,
  notifyMicrophoneUnavailable,
  setState,
} from './recording-evidence';
import { validateExplicitBindingAfterInvalidation } from './recording-input-invalidation';
import { queueDefaultRebind } from './recording-rebind';
import { clearOwner } from './recording-stop';
import { stopTest } from './recording-test';

const LEVEL_EVENT_INTERVAL_MS = 50;

export function initializeListeners(context: RecordingContext): void {
  context.onOwnerDestroyed = () => stopForOwnerLifecycle(context);
  context.onOwnerRenderProcessGone = () => stopForOwnerLifecycle(context);
  context.onOwnerDidStartNavigation = (
    _event: Electron.Event,
    _url: string,
    _isInPlace: boolean,
    isMainFrame: boolean,
  ) => {
    if (isMainFrame) stopForOwnerLifecycle(context);
  };
  context.removeFrameListener = context.capture.onFrame((frame) => {
    const dictation =
      context.dictation?.captureId === frame.captureId
        ? context.dictation
        : context.drainingDictation?.captureId === frame.captureId
          ? context.drainingDictation
          : null;
    if (dictation !== null) {
      dictation.callbacks.onFrame(frame.samples, frame.rms);
      return;
    }
    if (frame.captureId !== context.activeCaptureId || context.state.status !== 'active') return;
    context.testObservedRms = Math.max(context.testObservedRms, frame.rms);
    context.testSampleCount += frame.samples.length;
    const now = Date.now();
    if (now - context.lastLevelEventAt < LEVEL_EVENT_INTERVAL_MS) return;
    context.lastLevelEventAt = now;
    context.events.send('recording:test-level', {
      captureId: frame.captureId,
      rms: frame.rms,
    });
  });
  context.removeDeviceListener = context.capture.onDevicesChanged((defaultInvalidated) => {
    if (context.disposed) return;
    if (context.settings.get().recording.preferredMicrophoneId !== null) {
      validateExplicitBindingAfterInvalidation(context);
      return;
    }
    refreshAfterInputInvalidation(context);
    if (!defaultInvalidated) invalidateDefaultEvidence(context);
  });
  context.removeDefaultInvalidationListener = context.capture.onDefaultInvalidated(
    (captureId, bindingGeneration) => {
      if (
        captureId !== context.activeCaptureId ||
        bindingGeneration < context.activeBindingGeneration ||
        bindingGeneration > context.activeBindingGeneration + 1
      ) {
        return;
      }
      if (context.activePreferredMicrophoneId !== null) {
        if (!context.activePreferredUnavailable) return;
        validateExplicitBindingAfterInvalidation(context);
        queueDefaultRebind(context, bindingGeneration);
        return;
      }
      refreshAfterInputInvalidation(context);
      invalidateDefaultEvidence(context);
      queueDefaultRebind(context, bindingGeneration);
    },
  );
  context.removeStopListener = context.capture.onUnexpectedStop((captureId, reason) => {
    if (captureId !== context.activeCaptureId) return;
    const dictation = context.dictation;
    context.activeCaptureId = null;
    context.activeCaptureKind = null;
    context.activePreferredMicrophoneId = null;
    context.activeCaptureActivated = false;
    context.activeExplicitDeviceAbsent = false;
    context.activePreferredUnavailable = false;
    context.pendingDefaultRebindGeneration = null;
    context.defaultRebindAttemptGeneration = null;
    context.defaultRebindFollowUp = false;
    context.dictation = null;
    clearOwner(context);
    context.permission.release(captureId);
    context.systemAudio?.release(captureId);
    if (reason === 'device-unavailable') notifyMicrophoneUnavailable(context);
    if (dictation !== null) {
      try {
        dictation.callbacks.onUnexpectedStop(reason);
      } catch {
        // Local capture ownership is already released; consumer failure cannot undo cleanup.
      }
      return;
    }
    setState(context, {
      status: 'unavailable',
      permission: context.permission.getStatus(),
      reason: reason === 'system-audio-unavailable' ? 'capture-unavailable' : reason,
    });
  });
}

function stopForOwnerLifecycle(context: RecordingContext): void {
  const ownerId = context.ownerWebContents?.id;
  if (ownerId !== undefined) void stopTest(context, ownerId);
}
