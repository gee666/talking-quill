import type { MicrophoneDeviceList } from '../../shared/schemas/audio';
import type { RecordingContext } from './recording-context';

// Snapshot publication reads current evidence without triggering device enumeration.
export function deviceSnapshot(context: RecordingContext): MicrophoneDeviceList {
  const preferredMicrophoneId = context.settings.get().recording.preferredMicrophoneId;
  return {
    devices: [...context.devices],
    preferredMicrophoneId,
    preferredAvailable:
      preferredMicrophoneId === null ||
      (context.welcomeMicrophoneBindingKnown &&
        (context.devices.some((device) => device.deviceId === preferredMicrophoneId) ||
          (context.activePreferredMicrophoneId === preferredMicrophoneId &&
            !context.activeExplicitDeviceAbsent))),
    permission: context.permission.getStatus(),
  };
}

export function publishDeviceSnapshot(context: RecordingContext): void {
  const snapshot = deviceSnapshot(context);
  if (deviceSnapshotsEqual(context.lastPublishedDeviceSnapshot, snapshot)) return;
  context.lastPublishedDeviceSnapshot = snapshot;
  context.events.send('recording:devices-changed', snapshot);
}

function deviceSnapshotsEqual(
  first: MicrophoneDeviceList | null,
  second: MicrophoneDeviceList,
): boolean {
  if (
    first?.preferredMicrophoneId !== second.preferredMicrophoneId ||
    first.preferredAvailable !== second.preferredAvailable ||
    first.permission !== second.permission ||
    first.devices.length !== second.devices.length
  ) {
    return false;
  }
  return first.devices.every((device, index) => {
    const other = second.devices[index];
    return (
      device.deviceId === other?.deviceId &&
      device.label === other.label &&
      device.isDefault === other.isDefault
    );
  });
}
