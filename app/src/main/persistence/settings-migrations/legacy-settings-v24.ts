import { z } from 'zod';
import { LegacyRecordingSettingsSchema } from './legacy-settings-contracts';
import {
  LegacyDictationProfileListV27Schema,
  LegacySettingsV27ObjectSchema,
} from './legacy-settings-v27';

// V24 is the last released contract before manual submission and system-audio capture settings.
export const LegacySettingsV24Schema = LegacySettingsV27ObjectSchema.omit({
  schemaVersion: true,
  recording: true,
  dictationProfiles: true,
}).extend({
  schemaVersion: z.literal(24),
  recording: LegacyRecordingSettingsSchema,
  dictationProfiles: LegacyDictationProfileListV27Schema,
});

export type LegacySettingsV24 = z.infer<typeof LegacySettingsV24Schema>;
