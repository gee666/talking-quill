import { z } from 'zod';
import {
  LEGACY_SETTINGS_V19_V20_FIELDS,
  LEGACY_WELCOME_V19_V20_FIELDS,
  validateLegacySettingsMirrors,
} from './legacy-settings-v19';

// Frozen snapshot of the v20 on-disk contract. V20 removed the Welcome shortcut step but still
// stored profile activationKey/shift pairs and the app.activationKey compatibility mirror.
const LegacyWelcomeSettingsV20Schema = z
  .object({
    ...LEGACY_WELCOME_V19_V20_FIELDS,
    lastStep: z.union([z.literal(1), z.literal(2), z.literal(3), z.literal(4), z.literal(5)]),
  })
  .strict();

export const LegacySettingsV20Schema = z
  .object({
    schemaVersion: z.literal(20),
    ...LEGACY_SETTINGS_V19_V20_FIELDS,
    welcome: LegacyWelcomeSettingsV20Schema,
  })
  .strict()
  .superRefine(validateLegacySettingsMirrors);

export type LegacySettingsV20 = z.infer<typeof LegacySettingsV20Schema>;
