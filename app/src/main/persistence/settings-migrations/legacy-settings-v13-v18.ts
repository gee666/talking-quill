import { z } from 'zod';
import {
  LegacyExtendedAppSettingsSchema,
  LegacyPrivacyV10Schema,
  LegacyPrivacyV14Schema,
  LegacyRecordingSettingsSchema,
  LegacyRequiredLaunchAppSettingsSchema,
  LegacySmartProcessingPrePiPathSchema,
  LegacySmartProcessingSettingsSchema,
  LegacySmartProcessingV14Schema,
  LegacyTranscriptionSettingsSchema,
  LegacyVocabularyListSchema,
  LegacyVoiceCommandListSchema,
  LegacyWelcomeSettingsSchema,
  LegacyWelcomeSettingsV16Schema,
} from './legacy-settings-contracts';

const LegacySettingsV13Schema = z
  .object({
    schemaVersion: z.literal(13),
    app: LegacyRequiredLaunchAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV10Schema,
    smartProcessing: LegacySmartProcessingPrePiPathSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();
const LegacySettingsV14Schema = z
  .object({
    schemaVersion: z.literal(14),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingV14Schema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();
const LegacySettingsV15Schema = z
  .object({
    schemaVersion: z.literal(15),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingPrePiPathSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();
const LegacySettingsV16Schema = z
  .object({
    schemaVersion: z.literal(16),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsV16Schema,
  })
  .strict();
const LegacySettingsV17Schema = z
  .object({
    schemaVersion: z.literal(17),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingSettingsSchema.extend({
      piExtensionsEnabled: z.boolean(),
    }),
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();
const LegacySettingsV18Schema = z
  .object({
    schemaVersion: z.literal(18),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV14Schema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();

export const LEGACY_SETTINGS_V13_TO_V18_SCHEMAS = Object.freeze({
  13: LegacySettingsV13Schema,
  14: LegacySettingsV14Schema,
  15: LegacySettingsV15Schema,
  16: LegacySettingsV16Schema,
  17: LegacySettingsV17Schema,
  18: LegacySettingsV18Schema,
});
