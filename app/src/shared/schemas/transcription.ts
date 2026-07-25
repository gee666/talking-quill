import { z } from 'zod';
import { WHISPER_SAMPLE_RATE } from '../constants/whisper';
import { WhisperModelIdSchema } from './model-manifest';
import { boundedUtf8String } from './text-bounds';

export const TRANSCRIPT_MAX_CHARACTERS = 1_000_000;
export const TRANSCRIPT_MAX_UTF8_BYTES = 1_000_000;
export const TranscriptTextSchema = boundedUtf8String(
  TRANSCRIPT_MAX_CHARACTERS,
  TRANSCRIPT_MAX_UTF8_BYTES,
  'Transcript exceeds the local size limit',
);

export const ModelStateSchema = z.enum([
  'missing',
  'checking',
  'downloading',
  'verifying',
  'installing',
  'paused',
  'ready',
  'corrupt',
  'offline',
  'error',
]);

export const ModelStatusSchema = z
  .object({
    modelId: WhisperModelIdSchema,
    state: ModelStateSchema,
    downloadedBytes: z.number().int().nonnegative(),
    totalBytes: z.number().int().positive(),
    detail: z.string().max(240).nullable(),
    repairable: z.boolean(),
  })
  .strict();

const ProgressAmountsSchema = z
  .object({
    downloadedBytes: z.number().int().nonnegative(),
    totalBytes: z.number().int().positive(),
  })
  .strict();

export const ModelDeleteResultSchema = z
  .object({
    outcome: z.enum(['deleted', 'in-use']),
    status: ModelStatusSchema,
  })
  .strict();

export const ModelProgressSchema = z
  .object({
    modelId: WhisperModelIdSchema,
    state: ModelStateSchema,
    file: ProgressAmountsSchema.extend({ path: z.string().min(1).max(160) }).nullable(),
    total: ProgressAmountsSchema,
  })
  .strict();

export const TranscriptionOptionsSchema = z
  .object({
    modelId: WhisperModelIdSchema,
    sampleRate: z.literal(WHISPER_SAMPLE_RATE).default(WHISPER_SAMPLE_RATE),
    language: z.string().trim().min(2).max(80).optional(),
  })
  .strict();

export const PipelineReuseMetadataSchema = z
  .object({
    loadCount: z.number().int().positive(),
    reused: z.boolean(),
    loadDurationMs: z.number().nonnegative(),
  })
  .strict();

export const TranscriptionResultSchema = z
  .object({
    text: TranscriptTextSchema,
    modelId: WhisperModelIdSchema,
    durationMs: z.number().nonnegative(),
    pipeline: PipelineReuseMetadataSchema,
  })
  .strict();

export type ModelState = z.infer<typeof ModelStateSchema>;
export type ModelStatus = z.infer<typeof ModelStatusSchema>;
export type ModelProgress = z.infer<typeof ModelProgressSchema>;
export type ModelDeleteResult = z.infer<typeof ModelDeleteResultSchema>;
export type TranscriptionOptions = z.infer<typeof TranscriptionOptionsSchema>;
export type PipelineReuseMetadata = z.infer<typeof PipelineReuseMetadataSchema>;
export type TranscriptionResult = z.infer<typeof TranscriptionResultSchema>;
