import { randomUUID } from 'node:crypto';
import { CaptureClientError } from './capture-client-error';
import type {
  ActiveDictation,
  DictationCapture,
  DictationCaptureCallbacks,
  DictationCaptureOptions,
  RecordingContext,
} from './recording-context';
import {
  invalidateEvidenceForStartupFailure,
  notifyMicrophoneUnavailable,
  setState,
  setWelcomeMicrophoneBindingKnown,
} from './recording-evidence';
import { activateWithDeviceRefresh } from './recording-lifecycle';
import { enqueue } from './recording-operations';
import { stopActive } from './recording-stop';

export async function startDictation(
  context: RecordingContext,
  callbacks: DictationCaptureCallbacks,
  options: DictationCaptureOptions = {},
): Promise<DictationCapture> {
  const includeSystemAudio = options.includeSystemAudio === true;
  const operationGeneration = ++context.operationGeneration;
  context.pendingDictationGeneration = operationGeneration;
  const previousStop = stopActive(context);
  const result: { value: DictationCapture | null } = { value: null };
  try {
    await enqueue(context, async () => {
      if (context.disposed || operationGeneration !== context.operationGeneration) return;
      if (!(await previousStop) || operationGeneration !== context.operationGeneration) return;
      if (context.state.status === 'active' || context.state.status === 'starting') {
        setState(context, { status: 'idle', permission: context.permission.getStatus() });
      }
      const captureWebContents = context.captureWebContents;
      if (captureWebContents === null || captureWebContents.isDestroyed()) {
        throw new CaptureClientError('capture-unavailable');
      }
      const status = context.permission.getStatus();
      if (status === 'denied' || status === 'restricted') {
        throw new CaptureClientError('permission-denied');
      }
      const captureId = randomUUID();
      const preferredMicrophoneId = context.settings.get().recording.preferredMicrophoneId;
      context.activeCaptureId = captureId;
      context.activeCaptureKind = 'dictation';
      context.activePreferredMicrophoneId = preferredMicrophoneId;
      context.activeBindingGeneration = 0;
      context.activeCaptureActivated = false;
      context.activeExplicitDeviceAbsent = false;
      context.activePreferredUnavailable = false;
      context.permission.authorize(
        captureWebContents.id,
        captureId,
        preferredMicrophoneId === null ? 1 : 2,
      );
      try {
        if (includeSystemAudio) {
          if (context.systemAudio?.supported !== true) {
            throw new CaptureClientError('system-audio-unavailable');
          }
          context.systemAudio.authorize(captureWebContents, captureId);
        }
        const started = await context.capture.start(
          preferredMicrophoneId,
          captureId,
          includeSystemAudio,
        );
        if (operationGeneration !== context.operationGeneration) {
          await stopActive(context);
          return;
        }
        const dictation: ActiveDictation = {
          captureId,
          activeMicrophoneId: started.activeMicrophoneId,
          preferredUnavailable: started.preferredUnavailable,
          bindingGeneration: started.bindingGeneration,
          callbacks,
        };
        context.dictation = dictation;
        context.activeBindingGeneration = started.bindingGeneration;
        context.activePreferredUnavailable =
          preferredMicrophoneId !== null && started.preferredUnavailable;
        if (context.activePreferredUnavailable) {
          context.explicitValidationGeneration += 1;
          context.activeExplicitDeviceAbsent = true;
          setWelcomeMicrophoneBindingKnown(context, false);
          notifyMicrophoneUnavailable(context);
        } else if (
          preferredMicrophoneId === null ||
          started.activeMicrophoneId === preferredMicrophoneId
        ) {
          if (preferredMicrophoneId !== null) context.explicitValidationGeneration += 1;
          context.activeExplicitDeviceAbsent = false;
          setWelcomeMicrophoneBindingKnown(context, true);
        }
        const activated = await activateWithDeviceRefresh(
          context,
          captureWebContents.id,
          captureId,
          operationGeneration,
        );
        if (!activated || context.dictation !== dictation) {
          await stopActive(context);
          return;
        }
        result.value = {
          captureId,
          activeMicrophoneId: started.activeMicrophoneId,
          preferredUnavailable: started.preferredUnavailable,
        };
      } catch (error: unknown) {
        invalidateEvidenceForStartupFailure(context, error);
        const policyDenied =
          error instanceof CaptureClientError &&
          error.code === 'permission-denied' &&
          context.permission.takePolicyDenial(captureId);
        await stopActive(context);
        if (policyDenied) {
          console.error('Talking Quill microphone request rejected by application policy', {
            code: 'MICROPHONE_POLICY_DENIED',
          });
          throw new CaptureClientError('capture-unavailable');
        }
        throw error;
      }
    });
  } finally {
    if (context.pendingDictationGeneration === operationGeneration) {
      context.pendingDictationGeneration = null;
    }
  }
  if (result.value === null) throw new CaptureClientError('capture-unavailable');
  return result.value;
}

export async function stopDictation(context: RecordingContext, captureId?: string): Promise<void> {
  const activeDictationId =
    context.activeCaptureKind === 'dictation' ? context.activeCaptureId : null;
  const drainingDictationId = context.drainingDictation?.captureId ?? null;
  if (captureId !== undefined) {
    if (captureId === drainingDictationId) {
      await context.stopInFlight?.promise;
      return;
    }
    if (captureId !== activeDictationId) return;
  } else if (activeDictationId === null) {
    if (context.pendingDictationGeneration !== null) {
      const priorStop = context.stopInFlight?.promise;
      const cancellationGeneration = ++context.operationGeneration;
      context.pendingDictationGeneration = null;
      const safelyStopped = priorStop === undefined || (await priorStop);
      if (
        safelyStopped &&
        cancellationGeneration === context.operationGeneration &&
        (context.state.status === 'active' || context.state.status === 'starting')
      ) {
        setState(context, { status: 'idle', permission: context.permission.getStatus() });
      }
    } else if (drainingDictationId !== null) {
      await context.stopInFlight?.promise;
    }
    return;
  }
  ++context.operationGeneration;
  context.pendingDictationGeneration = null;
  await stopActive(context);
}
