import type { MicrophoneDevice, MicrophoneDeviceList } from '../../shared/schemas/audio';
import type {
  AuthorizedDeviceRefresh,
  DeviceRefreshWaiter,
  RecordingContext,
} from './recording-context';
import {
  invalidateActiveTestEvidence,
  notifyMicrophoneUnavailable,
  setWelcomeMicrophoneBindingKnown,
} from './recording-evidence';
import { withPermissionOperation } from './recording-operations';
import { deviceSnapshot, publishDeviceSnapshot } from './recording-snapshot';

export async function getDevices(context: RecordingContext): Promise<MicrophoneDeviceList> {
  await refreshDevices(context);
  return deviceSnapshot(context);
}

export function refreshAfterInputInvalidation(context: RecordingContext): Readonly<{
  generation: number;
  promise: Promise<void>;
}> {
  const captureId = context.activeCaptureId;
  const captureWebContents = context.captureWebContents;
  const canAuthorize =
    captureId !== null &&
    captureWebContents !== null &&
    !captureWebContents.isDestroyed() &&
    context.activeCaptureActivated;
  const promise = refreshDevices(
    context,
    canAuthorize
      ? {
          webContentsId: captureWebContents.id,
          captureId,
          operationGeneration: context.operationGeneration,
        }
      : undefined,
  );
  return { generation: context.deviceRefreshGeneration, promise };
}

export function refreshDevices(
  context: RecordingContext,
  authorized?: AuthorizedDeviceRefresh,
): Promise<void> {
  if (context.captureWebContents === null || context.disposed) return Promise.resolve();
  const generation = ++context.deviceRefreshGeneration;
  context.deviceRefreshDirty = true;
  if (authorized !== undefined) context.pendingAuthorizedDeviceRefresh = authorized;
  const result = new Promise<void>((resolve) => {
    context.deviceRefreshWaiters.push({ generation, resolve });
  });
  startDeviceRefreshDrain(context);
  return result;
}

function startDeviceRefreshDrain(context: RecordingContext): void {
  if (context.deviceRefreshInFlight !== null || context.disposed) return;
  const drain = drainDeviceRefreshes(context);
  context.deviceRefreshInFlight = drain;
  void drain.finally(() => {
    if (context.deviceRefreshInFlight === drain) context.deviceRefreshInFlight = null;
    if (context.deviceRefreshDirty) startDeviceRefreshDrain(context);
  });
}

async function drainDeviceRefreshes(context: RecordingContext): Promise<void> {
  while (context.deviceRefreshDirty && !context.disposed) {
    context.deviceRefreshDirty = false;
    const refreshGeneration = context.deviceRefreshGeneration;
    const attachmentGeneration = context.captureAttachmentGeneration;
    const captureWebContents = context.captureWebContents;
    const authorized = context.pendingAuthorizedDeviceRefresh;
    context.pendingAuthorizedDeviceRefresh = null;
    let result: Readonly<{ devices: readonly MicrophoneDevice[]; authorized: boolean }> | null =
      null;
    if (captureWebContents !== null) {
      try {
        if (authorized !== null && authorizedRefreshIsCurrent(context, authorized)) {
          result = await withPermissionOperation(context, async () => {
            if (!authorizedRefreshIsCurrent(context, authorized)) return null;
            context.permission.authorizeEnumeration(authorized.webContentsId, authorized.captureId);
            try {
              return { devices: await context.capture.listDevices(), authorized: true } as const;
            } finally {
              context.permission.seal(authorized.captureId);
            }
          });
        } else {
          result = await withPermissionOperation(context, async () => ({
            devices: await context.capture.listDevices(),
            authorized: false,
          }));
        }
      } catch {
        // Failed enumeration never replaces the last known authorized snapshot.
      }
    }
    if (
      result?.authorized === true &&
      refreshGeneration !== context.deviceRefreshGeneration &&
      authorized !== null &&
      authorizedRefreshIsCurrent(context, authorized)
    ) {
      retainAuthorizedDeviceRefresh(context, authorized);
    }
    const refreshIsCurrent = refreshGeneration === context.deviceRefreshGeneration;
    if (
      result !== null &&
      refreshIsCurrent &&
      (!result.authorized || authorizedRefreshIsCurrent(context, authorized)) &&
      attachmentGeneration === context.captureAttachmentGeneration &&
      captureWebContents === context.captureWebContents
    ) {
      if (result.authorized) {
        context.lastAuthorizedDeviceRefreshGeneration = refreshGeneration;
        context.devices = result.devices;
        const preferred = context.settings.get().recording.preferredMicrophoneId;
        if (preferred !== null) {
          const present = result.devices.some((device) => device.deviceId === preferred);
          const fallbackBinding =
            context.activePreferredMicrophoneId === preferred && context.activePreferredUnavailable;
          if (context.activePreferredMicrophoneId === preferred) {
            context.activeExplicitDeviceAbsent = fallbackBinding || !present;
          }
          if (fallbackBinding) {
            setWelcomeMicrophoneBindingKnown(context, false);
          } else if (present) {
            setWelcomeMicrophoneBindingKnown(context, true);
          } else {
            setWelcomeMicrophoneBindingKnown(context, false);
            invalidateActiveTestEvidence(context);
            notifyMicrophoneUnavailable(context);
          }
        }
      }
      publishDeviceSnapshot(context);
    }
    if (refreshIsCurrent) resolveDeviceRefreshWaiters(context, refreshGeneration);
  }
}

function retainAuthorizedDeviceRefresh(
  context: RecordingContext,
  refresh: AuthorizedDeviceRefresh,
): void {
  context.pendingAuthorizedDeviceRefresh ??= refresh;
}

function authorizedRefreshIsCurrent(
  context: RecordingContext,
  refresh: AuthorizedDeviceRefresh | null,
): boolean {
  return (
    refresh !== null &&
    !context.disposed &&
    refresh.operationGeneration === context.operationGeneration &&
    refresh.captureId === context.activeCaptureId &&
    context.captureWebContents?.id === refresh.webContentsId
  );
}

function resolveDeviceRefreshWaiters(context: RecordingContext, generation: number): void {
  const pending: DeviceRefreshWaiter[] = [];
  for (const waiter of context.deviceRefreshWaiters) {
    if (waiter.generation <= generation) waiter.resolve();
    else pending.push(waiter);
  }
  context.deviceRefreshWaiters = pending;
}
