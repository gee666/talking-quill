import {
  HistoryCreateSchema,
  type DictationMode,
  type HistoryCreate,
  type ProcessingMode,
} from '../../shared/schemas/history';

interface SessionHistoryBase {
  readonly createdAt: number;
  readonly dictationMode: DictationMode;
  readonly processingMode: ProcessingMode;
  readonly rawText: string;
}

export type SessionHistoryOutcome =
  | (SessionHistoryBase & { readonly kind: 'raw-completed' })
  | (SessionHistoryBase & {
      readonly kind: 'smart-completed';
      readonly processedText: string;
      readonly providerId: string | null;
      readonly modelId: string | null;
      readonly screenshotFilename?: string | null;
    })
  | (SessionHistoryBase & {
      readonly kind: 'smart-fallback';
      readonly providerId: string | null;
      readonly modelId: string | null;
      readonly errorCategory: string;
    })
  | (SessionHistoryBase & {
      readonly kind: 'voice-command';
      readonly voiceTrigger: string;
      readonly voiceSnippet: string;
    })
  | (SessionHistoryBase & {
      readonly kind: 'error';
      readonly errorCategory: string;
    });

export type SessionHistoryRecord = SessionHistoryOutcome extends infer Outcome
  ? Outcome extends SessionHistoryOutcome
    ? Omit<Outcome, 'createdAt'>
    : never
  : never;

export function mapSessionHistoryOutcome(outcome: SessionHistoryOutcome): HistoryCreate {
  const common = {
    createdAt: outcome.createdAt,
    dictationMode: outcome.dictationMode,
    processingMode: outcome.processingMode,
    rawText: outcome.rawText,
    processedText: null,
    providerId: null,
    modelId: null,
    fellBack: false,
    errorCategory: null,
    voiceTrigger: null,
    voiceSnippet: null,
    screenshotFilename: null,
  };

  switch (outcome.kind) {
    case 'raw-completed':
      return HistoryCreateSchema.parse({ ...common, outcome: 'raw-completed' });
    case 'smart-completed':
      return HistoryCreateSchema.parse({
        ...common,
        outcome: 'smart-completed',
        processedText: outcome.processedText,
        providerId: outcome.providerId,
        modelId: outcome.modelId,
        screenshotFilename: outcome.screenshotFilename ?? null,
      });
    case 'smart-fallback':
      return HistoryCreateSchema.parse({
        ...common,
        outcome: 'smart-fallback',
        providerId: outcome.providerId,
        modelId: outcome.modelId,
        fellBack: true,
        errorCategory: outcome.errorCategory,
      });
    case 'voice-command':
      return HistoryCreateSchema.parse({
        ...common,
        outcome: 'voice-command',
        voiceTrigger: outcome.voiceTrigger,
        voiceSnippet: outcome.voiceSnippet,
      });
    case 'error':
      return HistoryCreateSchema.parse({
        ...common,
        outcome: 'error',
        errorCategory: outcome.errorCategory,
      });
  }
}
