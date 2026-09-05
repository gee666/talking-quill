import type { WebContents } from 'electron';
import { CAPTURE_CANCEL_TIMEOUT_MS } from '../../shared/constants/audio';
import type { RecordingContext } from './recording-context';
import { setState } from './recording-evidence';

export function stopActive(context: RecordingContext, signal?: AbortSignal): Promise<boolean> {
  const inFlight = context.stopInFlight;
  if (inFlight !== null) return inFlight.promise;
  const captureId = context.activeCaptureId;
  const dictation = context.dictation;
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
  if (captureId === null) return Promise.resolve(true);
  context.activeCaptureId = null;
  if (dictation?.captureId === captureId) context.drainingDictation = dictation;
  context.permission.release(captureId);
  context.systemAudio?.release(captureId);

  const promise = stopCapture(context, captureId, signal);
  const stop = { promise };
  context.stopInFlight = stop;
  const clearStop = () => {
    if (context.stopInFlight === stop) context.stopInFlight = null;
    if (context.drainingDictation?.captureId === captureId) context.drainingDictation = null;
  };
  void promise.then(clearStop, clearStop);
  return promise;
}

async function stopCapture(
  context: RecordingContext,
  captureId: string,
  signal?: AbortSignal,
): Promise<boolean> {
  let resolveTimeout!: (value: false) => void;
  const timeout = new Promise<false>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(() => resolveTimeout(false), CAPTURE_CANCEL_TIMEOUT_MS);
  timer.unref();
  const stopping =
    signal === undefined
      ? context.capture.stop(captureId)
      : context.capture.stop(captureId, signal);
  const stopped = await Promise.race([
    stopping.then(
      () => true as const,
      () => false as const,
    ),
    timeout,
  ]);
  clearTimeout(timer);
  if (stopped) return true;
  forceCaptureReset(context, captureId);
  return false;
}

function forceCaptureReset(context: RecordingContext, captureId: string): void {
  try {
    context.capture.reset();
  } catch {
    // Continue clearing local ownership even if the failed transport cannot be reset cleanly.
  }
  const captureWebContents = context.captureWebContents;
  context.captureWebContents = null;
  if (captureWebContents !== null && !captureWebContents.isDestroyed()) {
    try {
      captureWebContents.reload();
    } catch {
      // The capture renderer may disappear between the destruction check and reload.
    }
  }
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
  context.drainingDictation = null;
  clearOwner(context);
  context.permission.release(captureId);
  context.systemAudio?.release(captureId);
  setState(context, {
    status: 'unavailable',
    permission: context.permission.getStatus(),
    reason: 'capture-unavailable',
  });
}

export function hasOwner(context: RecordingContext, ownerId: number): boolean {
  return context.ownerWebContents?.id === ownerId;
}

export function setOwner(context: RecordingContext, owner: WebContents): void {
  clearOwner(context);
  context.ownerWebContents = owner;
  owner.once('destroyed', context.onOwnerDestroyed);
  owner.on('did-start-navigation', context.onOwnerDidStartNavigation);
  owner.once('render-process-gone', context.onOwnerRenderProcessGone);
}

export function clearOwner(context: RecordingContext): void {
  context.ownerWebContents?.removeListener('destroyed', context.onOwnerDestroyed);
  context.ownerWebContents?.removeListener(
    'did-start-navigation',
    context.onOwnerDidStartNavigation,
  );
  context.ownerWebContents?.removeListener('render-process-gone', context.onOwnerRenderProcessGone);
  context.ownerWebContents = null;
}
