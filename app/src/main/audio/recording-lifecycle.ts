import type { WebContents } from 'electron';
import type { RecordingContext } from './recording-context';
import { refreshDevices } from './recording-devices';
import { enqueue } from './recording-operations';
import { startDefaultRebindDrain } from './recording-rebind';
import { stopActive } from './recording-stop';

export function attachCapture(context: RecordingContext, webContents: WebContents): void {
  context.captureAttachmentGeneration += 1;
  context.pendingAuthorizedDeviceRefresh = null;
  context.deviceRefreshDirty = false;
  context.deviceRefreshGeneration += 1;
  for (const waiter of context.deviceRefreshWaiters) waiter.resolve();
  context.deviceRefreshWaiters = [];
  context.captureWebContents = webContents;
  context.capture.attach(webContents);
}

export function shutdown(context: RecordingContext): Promise<void> {
  if (context.shutdownPromise !== null) return context.shutdownPromise;
  context.disposed = true;
  ++context.operationGeneration;
  ++context.deviceRefreshGeneration;
  context.deviceRefreshDirty = false;
  context.pendingDefaultRebindGeneration = null;
  context.defaultRebindAttemptGeneration = null;
  context.defaultRebindFollowUp = false;
  for (const waiter of context.deviceRefreshWaiters) waiter.resolve();
  context.deviceRefreshWaiters = [];
  const stopping = stopActive(context);
  context.shutdownPromise = (async () => {
    await enqueue(context, async () => {
      await stopping;
    });
    context.removeFrameListener();
    context.removeDeviceListener();
    context.removeDefaultInvalidationListener();
    context.removeStopListener();
    context.permission.releaseAll();
    context.systemAudio?.releaseAll();
    context.onMicrophoneUnavailable = null;
    context.onMicrophoneValidationChanged = null;
    context.capture.dispose();
  })();
  return context.shutdownPromise;
}

export async function activateWithDeviceRefresh(
  context: RecordingContext,
  webContentsId: number,
  captureId: string,
  operationGeneration: number,
  signal?: AbortSignal,
): Promise<boolean> {
  context.permission.seal(captureId);
  if (signal === undefined) await context.capture.activate(captureId);
  else await context.capture.activate(captureId, signal);
  if (
    context.disposed ||
    operationGeneration !== context.operationGeneration ||
    captureId !== context.activeCaptureId
  ) {
    return false;
  }
  context.activeCaptureActivated = true;
  startDefaultRebindDrain(context);
  void refreshDevices(context, { webContentsId, captureId, operationGeneration });
  return true;
}

export async function openMicrophoneSettings(context: RecordingContext): Promise<void> {
  await context.permission.openSettings();
}
