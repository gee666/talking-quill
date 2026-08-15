import { z } from 'zod';
import { MicrophoneIdSchema } from '../../../shared/schemas/audio';
import { RecordingSettingsSchema, SettingsObjectSchema } from '../../../shared/schemas/settings';

// V26 introduced Pi extension-source persistence. It still represented the system microphone
// default both as null and, in older persisted selections, as the literal pseudo-device ID.
export const LegacySettingsV26Schema = SettingsObjectSchema.omit({
  schemaVersion: true,
  recording: true,
}).extend({
  schemaVersion: z.literal(26),
  recording: RecordingSettingsSchema.extend({
    preferredMicrophoneId: MicrophoneIdSchema.nullable(),
  }),
});

export type LegacySettingsV26 = z.infer<typeof LegacySettingsV26Schema>;
