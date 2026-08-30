import { z } from 'zod';
import {
  LegacyDictationProfileListV27Schema,
  LegacySettingsV27ObjectSchema,
} from './legacy-settings-v27';

// V26 introduced Pi extension-source persistence. It still represented the system microphone
// default both as null and, in older persisted selections, as the literal pseudo-device ID.
export const LegacySettingsV26Schema = LegacySettingsV27ObjectSchema.omit({
  schemaVersion: true,
  recording: true,
  dictationProfiles: true,
}).extend({
  schemaVersion: z.literal(26),
  recording: LegacySettingsV27ObjectSchema.shape.recording.extend({
    preferredMicrophoneId: z.string().min(1).max(4_096).nullable(),
  }),
  dictationProfiles: LegacyDictationProfileListV27Schema,
});

export type LegacySettingsV26 = z.infer<typeof LegacySettingsV26Schema>;
