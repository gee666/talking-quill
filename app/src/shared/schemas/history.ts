import { z } from 'zod';
import { TranscriptTextSchema } from './transcription';

export const DictationModeSchema = z.enum(['quick', 'extended']);
export const ProcessingModeSchema = z.enum(['raw', 'smart']);
export const HistoryOutcomeSchema = z.enum([
  'raw-completed',
  'smart-completed',
  'smart-fallback',
  'voice-command',
  'error',
]);

const nullableText = TranscriptTextSchema.nullable();

export const HistoryCreateSchema = z
  .object({
    createdAt: z.number().int().nonnegative().optional(),
    dictationMode: DictationModeSchema,
    processingMode: ProcessingModeSchema,
    outcome: HistoryOutcomeSchema,
    rawText: nullableText,
    processedText: nullableText,
    providerId: z.string().max(128).nullable(),
    modelId: z.string().max(512).nullable(),
    fellBack: z.boolean(),
    errorCategory: z.string().max(128).nullable(),
    voiceTrigger: z.string().max(512).nullable(),
    voiceSnippet: nullableText,
    screenshotFilename: z
      .string()
      .max(255)
      .regex(/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/)
      .nullable(),
  })
  .strict();

export const HistoryIdSchema = z.uuid();

export const HistoryRecordSchema = HistoryCreateSchema.omit({ createdAt: true }).extend({
  id: HistoryIdSchema,
  createdAt: z.number().int().nonnegative(),
});

export const HistoryUpdateSchema = HistoryCreateSchema.omit({ createdAt: true })
  .partial()
  .refine((patch) => Object.keys(patch).length > 0, {
    message: 'At least one history field must be updated',
  });

export const HistoryCursorSchema = z
  .object({
    createdAt: z.number().int().nonnegative(),
    id: HistoryIdSchema,
  })
  .strict();

export const HistoryListItemSchema = HistoryRecordSchema.omit({ screenshotFilename: true }).extend({
  hasScreenshot: z.boolean(),
});
export const HistoryListRequestSchema = z
  .object({
    limit: z.number().int().min(1).max(50).optional(),
    cursor: HistoryCursorSchema.nullable().optional(),
  })
  .strict();
export const HistoryPageSchema = z
  .object({
    items: z.array(HistoryListItemSchema).max(50),
    nextCursor: HistoryCursorSchema.nullable(),
    revision: z.number().int().nonnegative(),
  })
  .strict();
export const HistoryCleanupStatusSchema = z.enum(['complete', 'pending', 'partial']);
export const HistoryDeleteResultSchema = z
  .object({
    deleted: z.boolean(),
    revision: z.number().int().nonnegative(),
    screenshotCleanup: HistoryCleanupStatusSchema,
  })
  .strict();
export const HistoryDeleteAllResultSchema = z
  .object({
    deletedCount: z.number().int().nonnegative(),
    revision: z.number().int().nonnegative(),
    screenshotCleanup: HistoryCleanupStatusSchema,
  })
  .strict();
export const HistoryRetentionResultSchema = z
  .object({
    deletedCount: z.number().int().nonnegative(),
    screenshotCleanup: HistoryCleanupStatusSchema,
  })
  .strict();
export const HistoryCopyResultSchema = z.object({ copied: z.literal(true) }).strict();
export const HistoryThumbnailSchema = z
  .object({
    base64: z
      .string()
      .regex(/^[A-Za-z0-9+/]*={0,2}$/)
      .max(480 * 1_024),
  })
  .strict()
  .nullable();
export const HistoryChangedSchema = z.object({ revision: z.number().int().nonnegative() }).strict();

export type DictationMode = z.infer<typeof DictationModeSchema>;
export type ProcessingMode = z.infer<typeof ProcessingModeSchema>;
export type HistoryCreate = z.infer<typeof HistoryCreateSchema>;
export type HistoryRecord = z.infer<typeof HistoryRecordSchema>;
export type HistoryUpdate = z.infer<typeof HistoryUpdateSchema>;
export type HistoryCursor = z.infer<typeof HistoryCursorSchema>;
export type HistoryListItem = z.infer<typeof HistoryListItemSchema>;
export type HistoryPage = z.infer<typeof HistoryPageSchema>;
export type HistoryCleanupStatus = z.infer<typeof HistoryCleanupStatusSchema>;
export type HistoryDeleteResult = z.infer<typeof HistoryDeleteResultSchema>;
export type HistoryDeleteAllResult = z.infer<typeof HistoryDeleteAllResultSchema>;
export type HistoryRetentionResult = z.infer<typeof HistoryRetentionResultSchema>;
