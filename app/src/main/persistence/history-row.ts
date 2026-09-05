import { HistoryRecordSchema, type HistoryRecord } from '../../shared/schemas/history';

export interface HistoryRow {
  readonly id: string;
  readonly created_at: number;
  readonly dictation_mode: string;
  readonly processing_mode: string;
  readonly outcome: string;
  readonly raw_text: string | null;
  readonly processed_text: string | null;
  readonly provider_id: string | null;
  readonly model_id: string | null;
  readonly fell_back: number;
  readonly error_category: string | null;
  readonly voice_trigger: string | null;
  readonly voice_snippet: string | null;
  readonly screenshot_filename: string | null;
}

export function mapRow(row: HistoryRow): HistoryRecord {
  return HistoryRecordSchema.parse({
    id: row.id,
    createdAt: row.created_at,
    dictationMode: row.dictation_mode,
    processingMode: row.processing_mode,
    outcome: row.outcome,
    rawText: row.raw_text,
    processedText: row.processed_text,
    providerId: row.provider_id,
    modelId: row.model_id,
    fellBack: row.fell_back === 1,
    errorCategory: row.error_category,
    voiceTrigger: row.voice_trigger,
    voiceSnippet: row.voice_snippet,
    screenshotFilename: row.screenshot_filename,
  });
}
