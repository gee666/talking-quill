import { z } from 'zod';
import { MicrophoneIdSchema } from '../../../shared/schemas/audio';
import {
  LegacyPiExtensionSourcesSchema,
  ProviderIdSchema,
} from '../../../shared/schemas/providers';
import {
  ProviderSettingsDraftSchema,
  RecordingSettingsSchema,
  SettingsObjectSchema,
  SmartProcessingSettingsSchema,
} from '../../../shared/schemas/settings';

export const LegacyV25LocalPiExtensionSourcesSchema = z
  .array(
    z
      .string()
      .trim()
      .min(1)
      .max(512)
      .regex(/^(?!-).+$/u)
      .refine(noControlCharacters)
      .refine(isLegacyLocalPiExtensionPath)
      .refine((value) => !/["%!&|<>^()]/u.test(value)),
  )
  .max(8);

const LegacyProviderSettingsDraftSchema = ProviderSettingsDraftSchema.extend({
  piExtensionSources: LegacyPiExtensionSourcesSchema.optional(),
});
const LegacySmartProcessingSettingsSchema = SmartProcessingSettingsSchema.extend({
  providers: z.partialRecord(ProviderIdSchema, LegacyProviderSettingsDraftSchema),
});

// Released v25 did not persist Pi extension sources. A short-lived prerelease used the same version
// with the optional provider-draft field, so this migration boundary deliberately accepts both shapes.
export const LegacySettingsV25Schema = SettingsObjectSchema.omit({
  schemaVersion: true,
  recording: true,
  smartProcessing: true,
}).extend({
  schemaVersion: z.literal(25),
  recording: RecordingSettingsSchema.extend({
    preferredMicrophoneId: MicrophoneIdSchema.nullable(),
  }),
  smartProcessing: LegacySmartProcessingSettingsSchema,
});

export type LegacySettingsV25 = z.infer<typeof LegacySettingsV25Schema>;

function noControlCharacters(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint < 0x20 || codePoint === 0x7f) return false;
  }
  return true;
}

function isLegacyLocalPiExtensionPath(value: string): boolean {
  if (/^(?:[\\/]{2}|[\\/]\?\?[\\/])/u.test(value)) return false;
  if (/^[A-Za-z]:[\\/]/u.test(value)) return true;
  return (
    !/^[A-Za-z][A-Za-z0-9+.-]*:/u.test(value) &&
    !value.startsWith('@') &&
    !/^[^\\/]+@[^\\/]+:/u.test(value)
  );
}
