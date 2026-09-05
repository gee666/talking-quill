import { z } from 'zod';
import {
  ProfileId,
  LegacyShortcutKeyV27Schema,
  LegacyDictationProfileListV27Schema,
} from './legacy-v27-profiles';
import { LegacySmartProcessingV27Schema } from './legacy-v27-providers';
import { VoiceCommands, VocabularyList } from './legacy-v27-text';
export {
  LegacyShortcutKeyV27Schema,
  LegacyShortcutV27Schema,
  type LegacyShortcutV27,
  LegacyDictationProfileV27Schema,
  LegacyDictationProfileListV27Schema,
} from './legacy-v27-profiles';
export {
  LegacyProviderIdV27Schema,
  LegacyProviderDraftV27Schema,
  LegacySmartProcessingV27Schema,
} from './legacy-v27-providers';

// Fully local snapshot of the released v27 on-disk contract. Nothing in this file imports a
// current settings, profile, provider, welcome, or shortcut schema, so future tightening cannot
// reinterpret the v27 migration boundary.
const ModelId = z.enum(['onnx-community/whisper-large-v3-turbo', 'Xenova/whisper-small']);
const TranscriptionLanguage = z.enum([
  'auto',
  'en',
  'zh',
  'de',
  'es',
  'ru',
  'ko',
  'fr',
  'ja',
  'pt',
  'tr',
  'pl',
  'ca',
  'nl',
  'ar',
  'sv',
  'it',
  'id',
  'hi',
  'fi',
  'vi',
  'he',
  'uk',
  'el',
  'ms',
  'cs',
  'ro',
  'da',
  'hu',
  'ta',
  'no',
  'th',
  'ur',
  'hr',
  'bg',
  'lt',
  'la',
  'mi',
  'ml',
  'cy',
  'sk',
  'te',
  'fa',
  'lv',
  'bn',
  'sr',
  'az',
  'sl',
  'kn',
  'et',
  'mk',
  'br',
  'eu',
  'is',
  'hy',
  'ne',
  'mn',
  'bs',
  'kk',
  'sq',
  'sw',
  'gl',
  'mr',
  'pa',
  'si',
  'km',
  'sn',
  'yo',
  'so',
  'af',
  'oc',
  'ka',
  'be',
  'tg',
  'sd',
  'gu',
  'am',
  'yi',
  'lo',
  'uz',
  'fo',
  'ht',
  'ps',
  'tk',
  'nn',
  'mt',
  'sa',
  'lb',
  'my',
  'bo',
  'tl',
  'mg',
  'as',
  'tt',
  'haw',
  'ln',
  'ha',
  'ba',
  'jw',
  'su',
]);
const MicrophoneEvidence = z
  .object({
    boundDeviceId: z.string().min(1).max(4096).nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold);
const ActivationEvidence = z
  .object({
    profileId: ProfileId,
    activationKey: LegacyShortcutKeyV27Schema,
    shift: z.boolean(),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const ModelEvidence = z
  .object({
    modelId: ModelId,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
const Welcome = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: z.union([z.literal(1), z.literal(2), z.literal(3), z.literal(4), z.literal(5)]),
    microphoneTested: z.boolean(),
    activationTested: z.boolean(),
    microphoneEvidence: MicrophoneEvidence.nullable().optional(),
    activationEvidence: ActivationEvidence.nullable().optional(),
    modelEvidence: ModelEvidence.nullable().optional(),
    revision: z.number().int().nonnegative().optional(),
  })
  .strict();

export const LegacySettingsV27ObjectSchema = z
  .object({
    schemaVersion: z.literal(27),
    app: z
      .object({
        enabled: z.boolean(),
        closeToTray: z.boolean(),
        defaultProcessingMode: z.enum(['raw', 'smart']),
        widgetSize: z.enum(['default', 'large', 'huge', 'max']),
        soundsEnabled: z.boolean(),
        launchAtLogin: z.boolean(),
      })
      .strict(),
    recording: z
      .object({
        preferredMicrophoneId: z
          .string()
          .min(1)
          .max(4096)
          .refine((id) => id !== 'default')
          .nullable(),
        silencePreset: z.enum(['aggressive', 'average', 'relaxed']),
        autoSubmitOnSilence: z.boolean(),
        includeSystemAudio: z.boolean(),
      })
      .strict(),
    transcription: z.object({ modelId: ModelId, language: TranscriptionLanguage }).strict(),
    dictationProfiles: LegacyDictationProfileListV27Schema,
    privacy: z
      .object({
        historyEnabled: z.boolean(),
        historyRetentionDays: z.union([z.literal(7), z.literal(30), z.literal(90)]).nullable(),
        retainSmartScreenshots: z.boolean(),
        diagnosticLoggingEnabled: z.boolean(),
      })
      .strict(),
    smartProcessing: LegacySmartProcessingV27Schema,
    voiceCommands: VoiceCommands,
    customVocabulary: VocabularyList,
    welcome: Welcome,
  })
  .strict();

export const LegacySettingsV27Schema = LegacySettingsV27ObjectSchema.superRefine(
  (settings, context) => {
    const general = settings.dictationProfiles.find(({ id }) => id === 'general');
    if (general !== undefined && general.processingMode !== settings.app.defaultProcessingMode) {
      context.addIssue({
        code: 'custom',
        path: ['app', 'defaultProcessingMode'],
        message: 'Processing mirror must match General',
      });
    }
  },
);

export type LegacySettingsV27 = z.infer<typeof LegacySettingsV27Schema>;
