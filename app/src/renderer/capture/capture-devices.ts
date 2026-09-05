import {
  MAX_MICROPHONE_DEVICES,
  MAX_MICROPHONE_ID_LENGTH,
  MAX_MICROPHONE_LABEL_LENGTH,
} from '../../shared/constants/audio';
import type { MicrophoneDevice } from '../../shared/schemas/audio';

export function sanitizeMicrophoneDevices(
  devices: readonly MediaDeviceInfo[],
): readonly MicrophoneDevice[] {
  const sanitized = new Map<string, MicrophoneDevice>();
  let anonymousIndex = 0;
  for (const device of devices) {
    if (device.kind !== 'audioinput') continue;
    const deviceId = sanitizeDeviceId(device.deviceId);
    if (deviceId === null || sanitized.has(deviceId)) continue;
    anonymousIndex += 1;
    sanitized.set(deviceId, {
      deviceId,
      label: sanitizeDeviceLabel(device.label, anonymousIndex),
      isDefault: deviceId === 'default',
    });
  }
  return [...sanitized.values()]
    .sort((first, second) => {
      if (first.isDefault !== second.isDefault) return first.isDefault ? -1 : 1;
      return (
        first.label.localeCompare(second.label) || first.deviceId.localeCompare(second.deviceId)
      );
    })
    .slice(0, MAX_MICROPHONE_DEVICES);
}

export function sanitizeDeviceId(deviceId: string | undefined): string | null {
  if (
    deviceId === undefined ||
    deviceId.trim().length === 0 ||
    deviceId.length > MAX_MICROPHONE_ID_LENGTH ||
    /\p{Cc}/u.test(deviceId)
  ) {
    return null;
  }
  return deviceId;
}

function sanitizeDeviceLabel(label: string, anonymousIndex: number): string {
  const sanitized = label
    .replace(/\p{Cc}/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim()
    .slice(0, MAX_MICROPHONE_LABEL_LENGTH)
    .trim();
  return sanitized || `Microphone ${String(anonymousIndex)}`;
}
