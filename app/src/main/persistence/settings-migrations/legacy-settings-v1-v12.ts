import { z } from 'zod';
import {
  LegacyAppSettingsSchema,
  LegacyCredentialEpochsSchema,
  LegacyExtendedAppSettingsSchema,
  LegacyLooseVocabularyListSchema,
  LegacyLooseVoiceCommandListSchema,
  LegacyPrivacyV10Schema,
  LegacyPrivacyV7Schema,
  LegacyProviderDraftsSchema,
  LegacyProviderDraftsV2Schema,
  LegacyProviderIdSchema,
  LegacyRecordingSettingsSchema,
  LegacyTranscriptionSettingsSchema,
  LegacyVocabularyListSchema,
  LegacyVoiceCommandListSchema,
} from './legacy-settings-contracts';

const LegacySmartProcessingV2Schema = z
  .object({
    selectedProviderId: LegacyProviderIdSchema,
    providers: LegacyProviderDraftsV2Schema,
  })
  .strict();
const LegacySmartProcessingV3Schema = z
  .object({
    selectedProviderId: LegacyProviderIdSchema,
    providers: LegacyProviderDraftsSchema,
  })
  .strict();
const LegacySmartProcessingV6Schema = LegacySmartProcessingV3Schema.extend({
  credentialEpochs: LegacyCredentialEpochsSchema,
});
const LegacySmartProcessingV10Schema = LegacySmartProcessingV6Schema.extend({
  onScreenAwarenessEnabled: z.boolean(),
  visionOverrides: z
    .array(
      z
        .object({
          providerId: z.enum(['generic-openai', 'litellm']),
          binding: z.string().min(1).max(2_048),
          modelId: z.string().trim().min(1).max(512),
          verifiedAt: z.number().int().nonnegative(),
        })
        .strict(),
    )
    .max(64),
});

const LegacySettingsV1Schema = z
  .object({ schemaVersion: z.literal(1), app: LegacyAppSettingsSchema })
  .strict();
const Task4SettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
  })
  .strict();
const Task9SettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    smartProcessing: LegacySmartProcessingV2Schema,
  })
  .strict();
const CombinedSettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    smartProcessing: LegacySmartProcessingV2Schema,
  })
  .strict();
const LegacySettingsV2Schema = z.union([
  CombinedSettingsV2Schema,
  Task4SettingsV2Schema,
  Task9SettingsV2Schema,
]);
const LegacySettingsV3Schema = z
  .object({
    schemaVersion: z.literal(3),
    app: LegacyAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    smartProcessing: LegacySmartProcessingV3Schema,
  })
  .strict();

// Two completed topics independently emitted schema version 4. The hybrid is a released,
// defensively integrated shape too, so all three contracts remain explicit.
const Task6SettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingV2Schema,
  })
  .strict();
const Task11SettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: LegacyAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    smartProcessing: LegacySmartProcessingV3Schema,
  })
  .strict();
const HybridSettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingV3Schema,
  })
  .strict();
const LegacySettingsV4Schema = z.union([
  HybridSettingsV4Schema,
  Task6SettingsV4Schema,
  Task11SettingsV4Schema,
]);
const LegacySettingsV5Schema = z
  .object({
    schemaVersion: z.literal(5),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingV3Schema,
  })
  .strict();
const LegacySettingsV6Schema = z
  .object({
    schemaVersion: z.literal(6),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingV6Schema,
  })
  .strict();

const Task7SettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV7Schema,
    smartProcessing: LegacySmartProcessingV6Schema,
  })
  .strict();
const Task8SettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingV6Schema,
    voiceCommands: LegacyLooseVoiceCommandListSchema,
    customVocabulary: LegacyLooseVocabularyListSchema,
  })
  .strict();
const HybridSettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV7Schema,
    smartProcessing: LegacySmartProcessingV6Schema,
    voiceCommands: LegacyLooseVoiceCommandListSchema,
    customVocabulary: LegacyLooseVocabularyListSchema,
  })
  .strict();
const LegacySettingsV7Schema = z.union([
  HybridSettingsV7Schema,
  Task7SettingsV7Schema,
  Task8SettingsV7Schema,
]);
const LegacySettingsV8Schema = z
  .object({
    schemaVersion: z.literal(8),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV7Schema,
    smartProcessing: LegacySmartProcessingV6Schema,
    voiceCommands: LegacyLooseVoiceCommandListSchema,
    customVocabulary: LegacyLooseVocabularyListSchema,
  })
  .strict();
const LegacySettingsV9Schema = LegacySettingsV8Schema.extend({ schemaVersion: z.literal(9) });
const LegacySettingsV10Schema = z
  .object({
    schemaVersion: z.literal(10),
    app: LegacyExtendedAppSettingsSchema,
    recording: LegacyRecordingSettingsSchema,
    transcription: LegacyTranscriptionSettingsSchema,
    privacy: LegacyPrivacyV10Schema,
    smartProcessing: LegacySmartProcessingV10Schema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
  })
  .strict();
const LegacyWelcomeProgressSchema = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: z.union([
      z.literal(1),
      z.literal(2),
      z.literal(3),
      z.literal(4),
      z.literal(5),
      z.literal(6),
    ]),
  })
  .strict();
const LegacySettingsV11Schema = LegacySettingsV10Schema.extend({
  schemaVersion: z.literal(11),
  welcome: LegacyWelcomeProgressSchema,
});
const LegacySettingsV12Schema = LegacySettingsV10Schema.extend({
  schemaVersion: z.literal(12),
  welcome: LegacyWelcomeProgressSchema.extend({
    microphoneTested: z.boolean(),
    activationTested: z.boolean(),
  }),
});

export const LEGACY_SETTINGS_V1_TO_V12_SCHEMAS = Object.freeze({
  1: LegacySettingsV1Schema,
  2: LegacySettingsV2Schema,
  3: LegacySettingsV3Schema,
  4: LegacySettingsV4Schema,
  5: LegacySettingsV5Schema,
  6: LegacySettingsV6Schema,
  7: LegacySettingsV7Schema,
  8: LegacySettingsV8Schema,
  9: LegacySettingsV9Schema,
  10: LegacySettingsV10Schema,
  11: LegacySettingsV11Schema,
  12: LegacySettingsV12Schema,
});
