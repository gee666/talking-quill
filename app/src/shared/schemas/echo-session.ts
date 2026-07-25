import { z } from 'zod';
import { DictationModeSchema, ProcessingModeSchema } from './history';
import { TranscriptTextSchema } from './transcription';

export const EchoAbortReasonSchema = z.enum([
  'user-cancel',
  'shutdown',
  'target-lost',
  'provider-error',
  'timeout',
]);
export const PiFallbackCategorySchema = z.enum([
  'pi-unavailable',
  'pi-authentication-failed',
  'pi-model-not-found',
  'pi-no-models',
  'pi-timeout',
  'pi-invalid-response',
  'pi-remote-failure',
]);
export const EchoSessionPhaseSchema = z.enum([
  'idle',
  'arming',
  'recordingQuick',
  'recordingExtended',
  'transcribing',
  'processingSmart',
  'inserting',
  'restoringClipboard',
  'completed',
  'cancelled',
  'error',
]);
export const EchoCompletionSchema = z.enum(['inserted', 'copied']);
export const EchoSessionSnapshotSchema = z
  .object({
    sessionId: z.uuid().nullable(),
    phase: EchoSessionPhaseSchema,
    dictationMode: DictationModeSchema.nullable(),
    processingMode: ProcessingModeSchema.nullable(),
    alternate: z.boolean(),
    rms: z.number().min(0).max(1),
    elapsedMs: z.number().int().nonnegative(),
    transcript: TranscriptTextSchema.nullable(),
    abortReason: EchoAbortReasonSchema.nullable(),
    fallbackCategory: PiFallbackCategorySchema.nullable(),
    completion: EchoCompletionSchema.nullable(),
    message: z.string().max(240).nullable(),
  })
  .strict();

export type EchoAbortReason = z.infer<typeof EchoAbortReasonSchema>;
export type PiFallbackCategory = z.infer<typeof PiFallbackCategorySchema>;
export type EchoSessionPhase = z.infer<typeof EchoSessionPhaseSchema>;
export type EchoSessionSnapshot = z.infer<typeof EchoSessionSnapshotSchema>;
