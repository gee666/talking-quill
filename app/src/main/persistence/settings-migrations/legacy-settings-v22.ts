import { z } from 'zod';
import {
  DictationProfileSchema,
  type DictationProfile,
} from '../../../shared/schemas/dictation-profiles';
import { SettingsObjectSchema } from '../../../shared/schemas/settings';
import { LegacyRecordingSettingsSchema } from './legacy-settings-contracts';

const LegacyBuiltInProfileIdV22Schema = z.enum([
  'general',
  'prompt',
  'markdown',
  'translate-to-english',
]);
const LegacyProfileIdV22Schema = z.union([LegacyBuiltInProfileIdV22Schema, z.uuid()]);
const LegacyDictationProfileV22Schema = DictationProfileSchema.extend({
  id: LegacyProfileIdV22Schema,
});
const LegacyDictationProfileListV22Schema = z
  .array(LegacyDictationProfileV22Schema)
  .min(4)
  .max(12)
  .superRefine((profiles, context) => {
    const ids = new Set<string>();
    for (const [index, profile] of profiles.entries()) {
      if (ids.has(profile.id)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'id'],
          message: 'Profile IDs must be unique',
        });
      }
      ids.add(profile.id);
    }
    for (const id of LegacyBuiltInProfileIdV22Schema.options) {
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
    }
  });

export const LegacySettingsV22Schema = SettingsObjectSchema.omit({
  schemaVersion: true,
  recording: true,
  dictationProfiles: true,
})
  .extend({
    schemaVersion: z.literal(22),
    recording: LegacyRecordingSettingsSchema,
    dictationProfiles: LegacyDictationProfileListV22Schema,
  })
  .superRefine((settings, context) => {
    const general = settings.dictationProfiles.find((profile) => profile.id === 'general');
    if (general !== undefined && settings.app.defaultProcessingMode !== general.processingMode) {
      context.addIssue({
        code: 'custom',
        path: ['app', 'defaultProcessingMode'],
        message: 'The processing mode compatibility mirror must match the General profile.',
      });
    }
  });

export type LegacySettingsV22 = Omit<
  z.infer<typeof LegacySettingsV22Schema>,
  'dictationProfiles'
> & {
  readonly dictationProfiles: readonly DictationProfile[];
};
