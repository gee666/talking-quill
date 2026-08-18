import { z } from 'zod';
import { VoiceCommandListSchema } from './commands';
import { DictationProfileListSchema } from './dictation-profiles';

export const SETTINGS_TRANSFER_FILE_MAX_BYTES = 1_048_576;

export const VoiceCommandsTransferSchema = z
  .object({
    format: z.literal('talking-quill.voice-commands'),
    version: z.literal(1),
    commands: VoiceCommandListSchema,
  })
  .strict();

export const DictationProfilesTransferSchema = z
  .object({
    format: z.literal('talking-quill.dictation-profiles'),
    version: z.literal(1),
    profiles: DictationProfileListSchema,
  })
  .strict();

export const SettingsTransferResultSchema = z.discriminatedUnion('status', [
  z.object({ status: z.literal('cancelled') }).strict(),
  z
    .object({ status: z.enum(['imported', 'exported']), count: z.number().int().nonnegative() })
    .strict(),
]);

export type SettingsTransferResult = z.infer<typeof SettingsTransferResultSchema>;
