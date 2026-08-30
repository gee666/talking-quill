import { z } from 'zod';
import { VoiceCommandListSchema } from './commands';
import { LegacyTransferV1ProfileListSchema } from './settings-transfer-v1';
import { LegacyTransferV2ProfileListSchema } from './settings-transfer-v2';

export const SETTINGS_TRANSFER_FILE_MAX_BYTES = 1_048_576;

export const VoiceCommandsTransferSchema = z
  .object({
    format: z.literal('talking-quill.voice-commands'),
    version: z.literal(1),
    commands: VoiceCommandListSchema,
  })
  .strict();

export const DictationProfilesTransferV1Schema = z
  .object({
    format: z.literal('talking-quill.dictation-profiles'),
    version: z.literal(1),
    profiles: LegacyTransferV1ProfileListSchema,
  })
  .strict();

export const DictationProfilesTransferV2Schema = z
  .object({
    format: z.literal('talking-quill.dictation-profiles'),
    version: z.literal(2),
    profiles: LegacyTransferV2ProfileListSchema,
  })
  .strict();

export const DictationProfilesTransferSchema = z.discriminatedUnion('version', [
  DictationProfilesTransferV1Schema,
  DictationProfilesTransferV2Schema,
]);

export const SettingsTransferResultSchema = z.discriminatedUnion('status', [
  z.object({ status: z.literal('cancelled') }).strict(),
  z
    .object({ status: z.enum(['imported', 'exported']), count: z.number().int().nonnegative() })
    .strict(),
]);

export type SettingsTransferResult = z.infer<typeof SettingsTransferResultSchema>;
