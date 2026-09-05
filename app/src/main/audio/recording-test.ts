import type { WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import type { MicrophoneTestState } from '../../shared/schemas/audio';
import type { RecordingContext } from './recording-context';
import {
  invalidateEvidenceForStartupFailure,
  notifyMicrophoneUnavailable,
  setFailureState,
  setState,
  setWelcomeMicrophoneBindingKnown,
} from './recording-evidence';
import { activateWithDeviceRefresh } from './recording-lifecycle';
import { enqueue } from './recording-operations';
import { hasOwner, setOwner, stopActive } from './recording-stop';

export async function startTest(
  context: RecordingContext,
  ownerWebContents: WebContents | null,
  signal?: AbortSignal,
): Promise<MicrophoneTestState> {
  if (
    signal?.aborted === true ||
    context.pendingDictationGeneration !== null ||
    context.activeCaptureKind === 'dictation' ||
    context.dictation !== null
  ) {
    return {
      status: 'unavailable',
      permission: context.permission.getStatus(),
      reason: 'capture-unavailable',
    };
  }
  const operationGeneration = ++context.operationGeneration;
  const previousStop = stopActive(context, signal);
  await enqueue(context, async () => {
    if (
      context.disposed ||
      signal?.aborted === true ||
      operationGeneration !== context.operationGeneration
    ) {
      return;
    }
    const previousStopped = await previousStop;
    if (!previousStopped || operationGeneration !== context.operationGeneration) return;
    const captureWebContents = context.captureWebContents;
    if (
      captureWebContents === null ||
      captureWebContents.isDestroyed() ||
      ownerWebContents === null ||
      ownerWebContents.isDestroyed()
    ) {
      setState(context, {
        status: 'unavailable',
        permission: context.permission.getStatus(),
        reason: 'capture-unavailable',
      });
      return;
    }
    const status = context.permission.getStatus();
    if (status === 'denied' || status === 'restricted') {
      setState(context, { status: 'blocked', permission: status, reason: 'microphone-permission' });
      return;
    }
    setOwner(context, ownerWebContents);
    setState(context, { status: 'starting', permission: status });
    const captureId = randomUUID();
    const preferredMicrophoneId = context.settings.get().recording.preferredMicrophoneId;
    context.activeCaptureId = captureId;
    context.activeCaptureKind = 'test';
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
      const started = await (signal === undefined
        ? context.capture.start(preferredMicrophoneId, captureId)
        : context.capture.start(preferredMicrophoneId, captureId, false, signal));
      if (
        operationGeneration !== context.operationGeneration ||
        !hasOwner(context, ownerWebContents.id)
      ) {
        await stopActive(context);
        return;
      }
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
        signal,
      );
      if (!activated || !hasOwner(context, ownerWebContents.id)) {
        await stopActive(context);
        return;
      }
      context.lastLevelEventAt = 0;
      context.testObservedRms = 0;
      context.testSampleCount = 0;
      setState(context, {
        status: 'active',
        permission: 'granted',
        captureId,
        activeMicrophoneId: started.activeMicrophoneId,
        preferredUnavailable: started.preferredUnavailable,
        bindingGeneration: started.bindingGeneration,
        sampleRate: started.sampleRate,
        channelCount: started.channelCount,
      });
    } catch (error: unknown) {
      invalidateEvidenceForStartupFailure(context, error);
      const safelyStopped = await stopActive(context, signal);
      if (safelyStopped && operationGeneration === context.operationGeneration) {
        setFailureState(context, error, captureId);
      }
    }
  });
  return getState(context);
}

export async function stopTest(
  context: RecordingContext,
  ownerWebContentsId?: number,
  signal?: AbortSignal,
): Promise<MicrophoneTestState> {
  if (
    context.pendingDictationGeneration !== null ||
    context.activeCaptureKind === 'dictation' ||
    context.dictation !== null
  ) {
    return getState(context);
  }
  if (
    ownerWebContentsId !== undefined &&
    context.ownerWebContents !== null &&
    ownerWebContentsId !== context.ownerWebContents.id
  ) {
    return getState(context);
  }
  const operationGeneration = ++context.operationGeneration;
  const safelyStopped = await stopActive(context, signal);
  if (safelyStopped && operationGeneration === context.operationGeneration) {
    setState(context, { status: 'idle', permission: context.permission.getStatus() });
  }
  return getState(context);
}

export function getState(context: RecordingContext): MicrophoneTestState {
  return structuredClone(context.state);
}

export function microphoneTestObservation(context: RecordingContext): {
  readonly boundDeviceId: string | null;
  readonly observedRms: number;
  readonly sampleCount: number;
} | null {
  if (
    context.state.status !== 'active' ||
    context.activeCaptureKind !== 'test' ||
    context.activeCaptureId !== context.state.captureId ||
    context.state.preferredUnavailable ||
    context.ownerWebContents === null ||
    context.activeBindingGeneration !== context.state.bindingGeneration ||
    context.settings.get().recording.preferredMicrophoneId !==
      context.activePreferredMicrophoneId ||
    (context.activePreferredMicrophoneId !== null &&
      (context.state.activeMicrophoneId !== context.activePreferredMicrophoneId ||
        context.activeExplicitDeviceAbsent))
  ) {
    return null;
  }
  return {
    boundDeviceId: context.state.activeMicrophoneId,
    observedRms: context.testObservedRms,
    sampleCount: context.testSampleCount,
  };
}
