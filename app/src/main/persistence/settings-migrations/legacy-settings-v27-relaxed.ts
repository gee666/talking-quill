import { LegacyTransferV2ProfileListSchema } from '../../../shared/schemas/settings-transfer-v2';
import { LegacySettingsV27ObjectSchema } from './legacy-settings-v27';

// Frozen snapshot of the short-lived development v27 shape that admitted shared-prefix profiles
// before those states received the v28 discriminator. This is a one-way rescue boundary only.
export const LegacySettingsV27RelaxedSchema = LegacySettingsV27ObjectSchema.extend({
  dictationProfiles: LegacyTransferV2ProfileListSchema,
}).superRefine((settings, context) => {
  const general = settings.dictationProfiles.find(({ id }) => id === 'general');
  if (general !== undefined && general.processingMode !== settings.app.defaultProcessingMode) {
    context.addIssue({
      code: 'custom',
      path: ['app', 'defaultProcessingMode'],
      message: 'Processing mirror must match General',
    });
  }
});
