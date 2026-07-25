import { z } from 'zod';
import {
  WHISPER_MAX_PUSH_SAMPLES,
  WHISPER_MAX_SAMPLES,
  WHISPER_PROTOCOL_VERSION,
} from '../constants/whisper';
import { TranscriptionOptionsSchema, TranscriptionResultSchema } from './transcription';

const RequestIdSchema = z.string().min(1).max(80);
const SessionIdSchema = z.string().min(1).max(80);
const PcmBufferSchema = z
  .instanceof(ArrayBuffer)
  .refine(
    (value) => value.byteLength > 0 && value.byteLength % Float32Array.BYTES_PER_ELEMENT === 0,
  )
  .refine(
    (value) => value.byteLength / Float32Array.BYTES_PER_ELEMENT <= WHISPER_MAX_SAMPLES,
    'PCM exceeds the maximum session duration',
  );
const PcmPushBufferSchema = PcmBufferSchema.refine(
  (value) => value.byteLength / Float32Array.BYTES_PER_ELEMENT <= WHISPER_MAX_PUSH_SAMPLES,
  'PCM push exceeds the maximum payload',
);

const envelope = {
  version: z.literal(WHISPER_PROTOCOL_VERSION),
  requestId: RequestIdSchema,
};

export const WhisperWorkerRequestSchema = z.discriminatedUnion('type', [
  z
    .object({
      ...envelope,
      type: z.literal('transcribe'),
      pcm: PcmBufferSchema,
      options: TranscriptionOptionsSchema,
    })
    .strict(),
  z
    .object({
      ...envelope,
      type: z.literal('session-open'),
      sessionId: SessionIdSchema,
      options: TranscriptionOptionsSchema,
    })
    .strict(),
  z
    .object({
      ...envelope,
      type: z.literal('session-push'),
      sessionId: SessionIdSchema,
      pcm: PcmPushBufferSchema,
    })
    .strict(),
  z.object({ ...envelope, type: z.literal('session-finish'), sessionId: SessionIdSchema }).strict(),
  z.object({ ...envelope, type: z.literal('session-cancel'), sessionId: SessionIdSchema }).strict(),
  z
    .object({
      ...envelope,
      type: z.literal('unload'),
      modelId: TranscriptionOptionsSchema.shape.modelId.optional(),
    })
    .strict(),
  z.object({ ...envelope, type: z.literal('health') }).strict(),
  z
    .object({
      ...envelope,
      type: z.literal('model-check'),
      modelId: TranscriptionOptionsSchema.shape.modelId,
    })
    .strict(),
  z.object({ ...envelope, type: z.literal('memory-pressure') }).strict(),
  z.object({ ...envelope, type: z.literal('shutdown') }).strict(),
]);

export const WhisperAcknowledgedOperationSchema = z.enum([
  'session-open',
  'session-push',
  'session-cancel',
  'unload',
  'memory-pressure',
  'shutdown',
]);

export const WhisperWorkerResultSchema = z.discriminatedUnion('type', [
  z
    .object({
      type: z.literal('ready'),
      networkGuarded: z.literal(true),
      networkProbeCompleted: z.boolean(),
    })
    .strict(),
  z.object({ type: z.literal('model-ready') }).strict(),
  z
    .object({
      type: z.literal('acknowledged'),
      operation: WhisperAcknowledgedOperationSchema,
    })
    .strict(),
  z.object({ type: z.literal('transcription'), value: TranscriptionResultSchema }).strict(),
]);

export const WhisperWorkerErrorCodeSchema = z.enum([
  'MODEL_MISSING',
  'MODEL_CORRUPT',
  'INVALID_AUDIO',
  'CANCELLED',
  'INFERENCE_FAILED',
  'PROTOCOL_ERROR',
]);

export const WhisperWorkerResponseSchema = z.discriminatedUnion('ok', [
  z.object({ ...envelope, ok: z.literal(true), result: WhisperWorkerResultSchema }).strict(),
  z
    .object({
      ...envelope,
      ok: z.literal(false),
      error: z
        .object({ code: WhisperWorkerErrorCodeSchema, message: z.string().min(1).max(240) })
        .strict(),
    })
    .strict(),
]);

export type WhisperAcknowledgedOperation = z.infer<typeof WhisperAcknowledgedOperationSchema>;
export type WhisperWorkerRequest = z.infer<typeof WhisperWorkerRequestSchema>;
export type WhisperWorkerResponse = z.infer<typeof WhisperWorkerResponseSchema>;
export type WhisperWorkerResult = z.infer<typeof WhisperWorkerResultSchema>;
export type WhisperWorkerErrorCode = z.infer<typeof WhisperWorkerErrorCodeSchema>;
