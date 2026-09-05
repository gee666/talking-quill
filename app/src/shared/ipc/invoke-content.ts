import { z } from 'zod';
import { MicrophoneDeviceListSchema, MicrophoneTestStateSchema } from '../schemas/audio';
import { EchoSessionSnapshotSchema } from '../schemas/echo-session';
import {
  VoiceCommandIdSchema,
  VoiceCommandInputSchema,
  VoiceCommandListSchema,
  VoiceCommandMatchSchema,
  VoiceCommandSchema,
  VoiceCommandUpdateSchema,
} from '../schemas/commands';
import {
  HistoryCopyResultSchema,
  HistoryDeleteAllResultSchema,
  HistoryDeleteResultSchema,
  HistoryIdSchema,
  HistoryListRequestSchema,
  HistoryPageSchema,
  HistoryThumbnailSchema,
} from '../schemas/history';
import {
  VocabularyEntrySchema,
  VocabularyFileResultSchema,
  VocabularyIdSchema,
  VocabularyListSchema,
  VocabularyValueSchema,
} from '../schemas/vocabulary';
import { WhisperModelIdSchema } from '../schemas/model-manifest';
import { SettingsTransferResultSchema } from '../schemas/settings-transfer';
import { ModelDeleteResultSchema, ModelStatusSchema } from '../schemas/transcription';
import { emptyRequest, acknowledgement, defineInvoke } from './invoke-definition';

export const contentInvokes = Object.freeze({
  'history:list': defineInvoke({
    roles: ['main'] as const,
    request: HistoryListRequestSchema,
    response: HistoryPageSchema,
  }),
  'history:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryDeleteResultSchema,
  }),
  'history:delete-all': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: HistoryDeleteAllResultSchema,
  }),
  'history:copy': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryCopyResultSchema,
  }),
  'history:thumbnail': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryThumbnailSchema,
  }),
  'model:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.array(ModelStatusSchema),
  }),
  'model:status': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema, verify: z.boolean().optional() }).strict(),
    response: ModelStatusSchema,
  }),
  'model:download': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:pause': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:cancel': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:retry': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelDeleteResultSchema,
  }),
  'window:minimize': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'window:toggle-maximize': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.object({ maximized: z.boolean() }).strict(),
  }),
  'window:close': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:ready': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: EchoSessionSnapshotSchema,
  }),
  'widget:stop': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:cancel': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:set-interactive': defineInvoke({
    roles: ['widget'] as const,
    request: z.object({ interactive: z.boolean() }).strict(),
    response: acknowledgement,
  }),
  'capture:ready': defineInvoke({
    roles: ['capture'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'recording:get-devices': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneDeviceListSchema,
  }),
  'recording:start-test': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneTestStateSchema,
  }),
  'recording:stop-test': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneTestStateSchema,
  }),
  'recording:open-microphone-settings': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'commands:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VoiceCommandListSchema,
  }),
  'commands:create': defineInvoke({
    roles: ['main'] as const,
    request: VoiceCommandInputSchema,
    response: VoiceCommandSchema,
  }),
  'commands:update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VoiceCommandIdSchema, patch: VoiceCommandUpdateSchema }).strict(),
    response: VoiceCommandSchema,
  }),
  'commands:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VoiceCommandIdSchema }).strict(),
    response: z.object({ deleted: z.boolean() }).strict(),
  }),
  'commands:preview': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ transcript: z.string().max(10_000) }).strict(),
    response: VoiceCommandMatchSchema.nullable(),
  }),
  'commands:import-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: SettingsTransferResultSchema,
  }),
  'commands:export-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: SettingsTransferResultSchema,
  }),
  'vocabulary:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyListSchema,
  }),
  'vocabulary:create': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ value: VocabularyValueSchema }).strict(),
    response: VocabularyEntrySchema,
  }),
  'vocabulary:update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VocabularyIdSchema, value: VocabularyValueSchema }).strict(),
    response: VocabularyEntrySchema,
  }),
  'vocabulary:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VocabularyIdSchema }).strict(),
    response: z.object({ deleted: z.boolean() }).strict(),
  }),
  'vocabulary:import-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyFileResultSchema,
  }),
  'vocabulary:export-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyFileResultSchema,
  }),
});
