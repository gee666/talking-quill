import { z } from 'zod';
import {
  DictationProfileSchema,
  type DictationProfile,
} from '../../../shared/schemas/dictation-profiles';
import { SettingsObjectSchema } from '../../../shared/schemas/settings';

const LegacyBuiltInProfileIdV23Schema = z.enum([
  'general',
  'prompt',
  'prompt-to-english',
  'markdown',
  'translate-to-english',
]);
const LegacyProfileIdV23Schema = z.union([LegacyBuiltInProfileIdV23Schema, z.uuid()]);
const LegacyDictationProfileV23Schema = DictationProfileSchema.extend({
  id: LegacyProfileIdV23Schema,
});
const LegacyDictationProfileListV23Schema = z
  .array(LegacyDictationProfileV23Schema)
  .min(5)
  .max(13)
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
    for (const id of LegacyBuiltInProfileIdV23Schema.options) {
      if (!ids.has(id))
        context.addIssue({ code: 'custom', message: `The ${id} profile is required` });
    }
  });

export const LegacySettingsV23Schema = SettingsObjectSchema.omit({
  schemaVersion: true,
  dictationProfiles: true,
})
  .extend({
    schemaVersion: z.literal(23),
    dictationProfiles: LegacyDictationProfileListV23Schema,
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

export type LegacySettingsV23 = Omit<
  z.infer<typeof LegacySettingsV23Schema>,
  'dictationProfiles'
> & {
  readonly dictationProfiles: readonly DictationProfile[];
};
