import { z } from 'zod';
import {
  MAX_MICROPHONE_DEVICES,
  MAX_MICROPHONE_ID_LENGTH,
  MAX_MICROPHONE_LABEL_LENGTH,
  PCM_CHANNEL_COUNT,
  PCM_SAMPLE_RATE,
} from '../constants/audio';

export const MicrophoneIdSchema = z.string().min(1).max(MAX_MICROPHONE_ID_LENGTH);
export const SilencePresetSchema = z.enum(['aggressive', 'average', 'relaxed']);
export const MicrophonePermissionStateSchema = z.enum([
  'not-determined',
  'granted',
  'denied',
  'restricted',
  'unavailable',
]);

export const MicrophoneDeviceSchema = z
  .object({
    deviceId: MicrophoneIdSchema,
    label: z.string().min(1).max(MAX_MICROPHONE_LABEL_LENGTH),
    isDefault: z.boolean(),
  })
  .strict();

export const MicrophoneDeviceListSchema = z
  .object({
    devices: z.array(MicrophoneDeviceSchema).max(MAX_MICROPHONE_DEVICES),
    preferredMicrophoneId: MicrophoneIdSchema.nullable(),
    preferredAvailable: z.boolean(),
    permission: MicrophonePermissionStateSchema,
  })
  .strict();

const IdleTestStateSchema = z
  .object({
    status: z.literal('idle'),
    permission: MicrophonePermissionStateSchema,
  })
  .strict();

const StartingTestStateSchema = z
  .object({
    status: z.literal('starting'),
    permission: MicrophonePermissionStateSchema,
  })
  .strict();

const ActiveTestStateSchema = z
  .object({
    status: z.literal('active'),
    permission: z.literal('granted'),
    captureId: z.uuid(),
    activeMicrophoneId: MicrophoneIdSchema.nullable(),
    preferredUnavailable: z.boolean(),
    sampleRate: z.literal(PCM_SAMPLE_RATE),
    channelCount: z.literal(PCM_CHANNEL_COUNT),
  })
  .strict();

const BlockedTestStateSchema = z
  .object({
    status: z.literal('blocked'),
    permission: z.enum(['denied', 'restricted']),
    reason: z.literal('microphone-permission'),
  })
  .strict();

const UnavailableTestStateSchema = z
  .object({
    status: z.literal('unavailable'),
    permission: MicrophonePermissionStateSchema,
    reason: z.enum([
      'no-device',
      'device-unavailable',
      'capture-unavailable',
      'permission-unavailable',
      'unsupported-audio-format',
    ]),
  })
  .strict();

export const MicrophoneTestStateSchema = z.discriminatedUnion('status', [
  IdleTestStateSchema,
  StartingTestStateSchema,
  ActiveTestStateSchema,
  BlockedTestStateSchema,
  UnavailableTestStateSchema,
]);

export const MicrophoneLevelSchema = z
  .object({
    captureId: z.uuid(),
    rms: z.number().min(0).max(1),
  })
  .strict();

export type SilencePreset = z.infer<typeof SilencePresetSchema>;
export type MicrophonePermissionState = z.infer<typeof MicrophonePermissionStateSchema>;
export type MicrophoneDevice = z.infer<typeof MicrophoneDeviceSchema>;
export type MicrophoneDeviceList = z.infer<typeof MicrophoneDeviceListSchema>;
export type MicrophoneTestState = z.infer<typeof MicrophoneTestStateSchema>;
export type MicrophoneLevel = z.infer<typeof MicrophoneLevelSchema>;
