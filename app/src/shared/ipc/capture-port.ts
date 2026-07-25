import { z } from 'zod';
import {
  CAPTURE_PORT_PROTOCOL_VERSION,
  MAX_MICROPHONE_DEVICES,
  PCM_CHANNEL_COUNT,
  PCM_FRAME_SAMPLES,
  PCM_SAMPLE_RATE,
} from '../constants/audio';
import { MicrophoneDeviceSchema, MicrophoneIdSchema } from '../schemas/audio';

const RequestIdSchema = z.uuid();
const CaptureIdSchema = z.uuid();

export const CapturePortDescriptorSchema = z
  .object({ protocolVersion: z.literal(CAPTURE_PORT_PROTOCOL_VERSION) })
  .strict();

export const CapturePortCommandSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('devices:list'), requestId: RequestIdSchema }).strict(),
  z
    .object({
      type: z.literal('stream:start'),
      requestId: RequestIdSchema,
      captureId: CaptureIdSchema,
      preferredMicrophoneId: MicrophoneIdSchema.nullable(),
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:activate'),
      requestId: RequestIdSchema,
      captureId: CaptureIdSchema,
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:stop'),
      requestId: RequestIdSchema,
      captureId: CaptureIdSchema,
    })
    .strict(),
]);

const PcmSamplesSchema = z
  .instanceof(Float32Array)
  .refine((samples) => samples.length > 0 && samples.length <= PCM_FRAME_SAMPLES, {
    message: 'Invalid PCM frame length',
  })
  .refine((samples) => {
    for (const sample of samples) {
      if (!Number.isFinite(sample) || sample < -1 || sample > 1) return false;
    }
    return true;
  }, 'PCM samples must be finite normalized values');

export const CapturePortMessageSchema = z.discriminatedUnion('type', [
  z
    .object({
      type: z.literal('port:ready'),
      protocolVersion: z.literal(CAPTURE_PORT_PROTOCOL_VERSION),
    })
    .strict(),
  z
    .object({
      type: z.literal('devices:list-result'),
      requestId: RequestIdSchema,
      devices: z.array(MicrophoneDeviceSchema).max(MAX_MICROPHONE_DEVICES),
    })
    .strict(),
  z
    .object({
      type: z.literal('devices:changed'),
      devices: z.array(MicrophoneDeviceSchema).max(MAX_MICROPHONE_DEVICES),
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:started'),
      requestId: RequestIdSchema,
      captureId: CaptureIdSchema,
      sampleRate: z.literal(PCM_SAMPLE_RATE),
      channelCount: z.literal(PCM_CHANNEL_COUNT),
      activeMicrophoneId: MicrophoneIdSchema.nullable(),
      preferredUnavailable: z.boolean(),
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:activated'),
      requestId: RequestIdSchema,
      captureId: CaptureIdSchema,
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:frame'),
      captureId: CaptureIdSchema,
      sequence: z.number().int().nonnegative(),
      samples: PcmSamplesSchema,
      rms: z.number().min(0).max(1),
    })
    .strict(),
  z
    .object({
      type: z.literal('stream:stopped'),
      requestId: RequestIdSchema.nullable(),
      captureId: CaptureIdSchema,
      reason: z.enum(['requested', 'device-lost', 'replaced', 'page-unload', 'error']),
    })
    .strict(),
  z
    .object({
      type: z.literal('request:error'),
      requestId: RequestIdSchema.nullable(),
      captureId: CaptureIdSchema.nullable(),
      code: z.enum([
        'permission-denied',
        'no-device',
        'device-unavailable',
        'unsupported-audio-format',
        'worklet-unavailable',
        'capture-failed',
      ]),
    })
    .strict(),
]);

export type CapturePortDescriptor = z.infer<typeof CapturePortDescriptorSchema>;
export type CapturePortCommand = z.infer<typeof CapturePortCommandSchema>;
export type CapturePortMessage = z.infer<typeof CapturePortMessageSchema>;
