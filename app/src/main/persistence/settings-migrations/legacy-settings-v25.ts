import { z } from 'zod';
import {
  LegacyDictationProfileListV27Schema,
  LegacyProviderDraftV27Schema,
  LegacyProviderIdV27Schema,
  LegacySettingsV27ObjectSchema,
  LegacySmartProcessingV27Schema,
} from './legacy-settings-v27';

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

const LegacyV25PiExtensionSourcesSchema = z
  .array(
    z
      .string()
      .trim()
      .min(1)
      .max(512)
      .regex(/^(?!-).+$/u)
      .refine(noControlCharacters),
  )
  .max(8);
const LegacyProviderSettingsDraftSchema = LegacyProviderDraftV27Schema.extend({
  piExtensionSources: LegacyV25PiExtensionSourcesSchema.optional(),
});
const LegacySmartProcessingSettingsSchema = LegacySmartProcessingV27Schema.extend({
  providers: z.partialRecord(LegacyProviderIdV27Schema, LegacyProviderSettingsDraftSchema),
});

// Released v25 did not persist Pi extension sources. A short-lived prerelease used the same version
// with the optional provider-draft field, so this migration boundary deliberately accepts both shapes.
export const LegacySettingsV25Schema = LegacySettingsV27ObjectSchema.omit({
  schemaVersion: true,
  recording: true,
  smartProcessing: true,
  dictationProfiles: true,
}).extend({
  schemaVersion: z.literal(25),
  recording: LegacySettingsV27ObjectSchema.shape.recording.extend({
    preferredMicrophoneId: z.string().min(1).max(4_096).nullable(),
  }),
  smartProcessing: LegacySmartProcessingSettingsSchema,
  dictationProfiles: LegacyDictationProfileListV27Schema,
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
