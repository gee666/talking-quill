import { z } from 'zod';
import { SettingsObjectSchema } from '../../../shared/schemas/settings';
import { LegacyRecordingSettingsSchema } from './legacy-settings-contracts';

// V24 is the last released contract before manual submission and system-audio capture settings.
export const LegacySettingsV24Schema = SettingsObjectSchema.omit({
  schemaVersion: true,
  recording: true,
}).extend({
  schemaVersion: z.literal(24),
  recording: LegacyRecordingSettingsSchema,
});

export type LegacySettingsV24 = z.infer<typeof LegacySettingsV24Schema>;
