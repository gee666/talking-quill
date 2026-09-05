import { DEFAULT_MICROPHONE_REBIND_ATTEMPTS } from '../../shared/constants/audio';
import { CaptureClientError } from './capture-client-error';
import type { RecordingContext } from './recording-context';
import {
  invalidateActiveTestEvidence,
  notifyMicrophoneUnavailable,
  setState,
} from './recording-evidence';
import { withPermissionOperation } from './recording-operations';
import { stopActive } from './recording-stop';

export function queueDefaultRebind(context: RecordingContext, bindingGeneration: number): void {
  if (
    (context.activePreferredMicrophoneId !== null && !context.activePreferredUnavailable) ||
    bindingGeneration < context.activeBindingGeneration
  ) {
    return;
  }
  if (context.defaultRebindAttemptGeneration === bindingGeneration) {
    context.defaultRebindFollowUp = true;
    return;
  }
  if (
    context.pendingDefaultRebindGeneration === null ||
    bindingGeneration > context.pendingDefaultRebindGeneration
  ) {
    context.pendingDefaultRebindGeneration = bindingGeneration;
  }
  if (context.activeCaptureActivated) startDefaultRebindDrain(context);
}

export function startDefaultRebindDrain(context: RecordingContext): void {
  if (
    context.defaultRebindInFlight !== null ||
    context.pendingDefaultRebindGeneration === null ||
    context.disposed
  ) {
    return;
  }
  const drain = drainDefaultRebinds(context);
  context.defaultRebindInFlight = drain;
  void drain.finally(() => {
    if (context.defaultRebindInFlight === drain) context.defaultRebindInFlight = null;
    if (context.pendingDefaultRebindGeneration !== null) startDefaultRebindDrain(context);
  });
}

async function drainDefaultRebinds(context: RecordingContext): Promise<void> {
  while (context.pendingDefaultRebindGeneration !== null && !context.disposed) {
    const bindingGeneration = context.pendingDefaultRebindGeneration;
    context.pendingDefaultRebindGeneration = null;
    const captureId = context.activeCaptureId;
    const captureWebContents = context.captureWebContents;
    const operationGeneration = context.operationGeneration;
    if (
      captureId === null ||
      captureWebContents === null ||
      captureWebContents.isDestroyed() ||
      !context.activeCaptureActivated ||
      (context.activePreferredMicrophoneId !== null && !context.activePreferredUnavailable) ||
      bindingGeneration !== context.activeBindingGeneration
    ) {
      continue;
    }
    context.defaultRebindAttemptGeneration = bindingGeneration;
    let failedAttempts = 0;
    try {
      while (failedAttempts < DEFAULT_MICROPHONE_REBIND_ATTEMPTS) {
        try {
          const rebound = await withPermissionOperation(context, async () => {
            if (
              context.disposed ||
              operationGeneration !== context.operationGeneration ||
              captureId !== context.activeCaptureId ||
              bindingGeneration !== context.activeBindingGeneration
            ) {
              throw new CaptureClientError('capture-unavailable');
            }
            context.permission.authorize(captureWebContents.id, captureId);
            try {
              return await context.capture.rebindDefault(captureId, bindingGeneration);
            } finally {
              context.permission.seal(captureId);
            }
          });
          if (
            operationGeneration !== context.operationGeneration ||
            captureId !== context.activeCaptureId ||
            bindingGeneration !== context.activeBindingGeneration
          ) {
            return;
          }
          if (rebound.bindingGeneration !== bindingGeneration + 1) {
            throw new CaptureClientError('capture-failed');
          }
          context.activeBindingGeneration = rebound.bindingGeneration;
          if (context.dictation?.captureId === captureId) {
            context.dictation = {
              ...context.dictation,
              activeMicrophoneId: rebound.activeMicrophoneId,
              bindingGeneration: rebound.bindingGeneration,
            };
          }
          if (context.state.status === 'active' && context.state.captureId === captureId) {
            invalidateActiveTestEvidence(context);
            setState(context, {
              ...context.state,
              activeMicrophoneId: rebound.activeMicrophoneId,
              bindingGeneration: rebound.bindingGeneration,
            });
          }
          notifyMicrophoneUnavailable(context);
          if (context.defaultRebindFollowUp) {
            context.defaultRebindFollowUp = false;
            context.pendingDefaultRebindGeneration = rebound.bindingGeneration;
          }
          break;
        } catch {
          if (
            operationGeneration !== context.operationGeneration ||
            captureId !== context.activeCaptureId
          ) {
            return;
          }
          failedAttempts += 1;
          if (failedAttempts >= DEFAULT_MICROPHONE_REBIND_ATTEMPTS) {
            await failDefaultRebind(context, captureId);
            return;
          }
        }
      }
    } finally {
      if (context.defaultRebindAttemptGeneration === bindingGeneration) {
        context.defaultRebindAttemptGeneration = null;
      }
    }
  }
}

async function failDefaultRebind(context: RecordingContext, captureId: string): Promise<void> {
  if (captureId !== context.activeCaptureId) return;
  const dictation = context.dictation;
  const wasTest = context.activeCaptureKind === 'test';
  const failureGeneration = ++context.operationGeneration;
  context.pendingDefaultRebindGeneration = null;
  context.defaultRebindFollowUp = false;
  await stopActive(context);
  if (failureGeneration !== context.operationGeneration) return;
  if (dictation !== null) {
    try {
      dictation.callbacks.onUnexpectedStop('device-unavailable');
    } catch {
      // Capture ownership is already released; consumer failure cannot undo cleanup.
    }
  } else if (wasTest) {
    setState(context, {
      status: 'unavailable',
      permission: context.permission.getStatus(),
      reason: 'device-unavailable',
    });
  }
}
