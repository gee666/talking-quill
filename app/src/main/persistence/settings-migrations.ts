import { z } from 'zod';
import {
  VOICE_COMMAND_LIMIT,
  VOICE_COMMANDS_MAX_UTF8_BYTES,
  VOICE_SNIPPET_MAX_LENGTH,
  VOICE_TRIGGER_MAX_LENGTH,
  VoiceCommandListSchema,
  VoiceCommandSchema,
  type VoiceCommand,
} from '../../shared/schemas/commands';
import { ProviderConfigSchema, ProviderIdSchema } from '../../shared/schemas/providers';
import {
  VOCABULARY_LIMIT,
  VOCABULARY_TOTAL_MAX_UTF8_BYTES,
  VOCABULARY_VALUE_MAX_LENGTH,
  VocabularyEntrySchema,
  VocabularyListSchema,
  type VocabularyEntry,
} from '../../shared/schemas/vocabulary';
import { utf8ByteLength } from '../../shared/schemas/text-bounds';
import {
  AppSettingsSchema as CurrentAppSettingsSchema,
  DEFAULT_SETTINGS,
  PrivacySettingsSchema,
  ProviderSettingsDraftSchema,
  RecordingSettingsSchema,
  SETTINGS_SCHEMA_VERSION,
  SettingsObjectSchema,
  SettingsSchema,
  SmartProcessingSettingsSchema,
} from '../../shared/schemas/settings';
import type { ProviderId } from '../../shared/schemas/providers';
import type { ProviderSettingsDraft, Settings } from '../../shared/schemas/settings';
import type { SettingsMigrations } from './settings-store';
import { WelcomeSettingsSchema } from '../../shared/schemas/welcome';
import { ActivationKeySchema } from '../../shared/helper/protocol';
import { ProcessingModeSchema } from '../../shared/schemas/history';
import { defaultDictationProfiles } from '../../shared/schemas/dictation-profiles';

const LegacyWhisperModelIdSchema = z.enum([
  'Xenova/whisper-small',
  'Xenova/whisper-large',
  'onnx-community/whisper-large-v3-turbo',
]);
const LegacyTranscriptionSettingsSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    language: z.string().trim().min(1).max(80).nullable(),
  })
  .strict();
// Historical schemas intentionally retain the removed large model so it can be migrated safely.
const TranscriptionSettingsSchema = LegacyTranscriptionSettingsSchema;
const LegacyModelEvidenceSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyActivationEvidenceSchema = z
  .object({
    activationKey: z.string().length(1),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyWelcomeSettingsSchema = WelcomeSettingsSchema.omit({ activationEvidence: true }).extend(
  {
    activationEvidence: LegacyActivationEvidenceSchema.nullable().optional(),
    modelEvidence: LegacyModelEvidenceSchema.nullable().optional(),
  },
);

const LegacyAppSettingsSchema = z
  .object({ enabled: z.boolean(), closeToTray: z.boolean() })
  .strict();
const AppSettingsSchema = CurrentAppSettingsSchema.extend({
  activationKey: ActivationKeySchema,
  defaultProcessingMode: ProcessingModeSchema,
  launchAtLogin: z.boolean().optional(),
});

const LegacyProviderDraftSchema = z
  .object({
    baseUrl: z.url().max(2_048).optional(),
    modelId: z.string().trim().min(1).max(512).nullable().optional(),
    contextWindow: z.number().int().min(1).max(2_000_000).optional(),
    maxOutputTokens: z.number().int().min(1).max(16_384).optional(),
    timeoutMs: z.number().int().min(500).max(120_000).optional(),
    keepAlive: z
      .union([
        z.number().int().min(-1).max(86_400),
        z
          .string()
          .trim()
          .min(1)
          .max(32)
          .regex(/^(?:\d+(?:\.\d+)?(?:ns|us|µs|ms|s|m|h))+$/u),
      ])
      .optional(),
  })
  .strict();

const LegacyProviderDraftsSchema = z
  .partialRecord(ProviderIdSchema, LegacyProviderDraftSchema)
  .superRefine(validateLegacyProviderDrafts);
const TransitionalProviderDraftsSchema = z
  .partialRecord(ProviderIdSchema, ProviderSettingsDraftSchema)
  .superRefine(validateLegacyProviderDrafts);

const LegacySmartProcessingSettingsSchema = z
  .object({
    selectedProviderId: ProviderIdSchema,
    providers: LegacyProviderDraftsSchema,
  })
  .strict();
const TransitionalSmartProcessingSettingsSchema = z
  .object({
    selectedProviderId: ProviderIdSchema,
    providers: TransitionalProviderDraftsSchema,
  })
  .strict();
const Task9SmartProcessingSettingsSchema = z
  .object({
    selectedProviderId: ProviderIdSchema,
    providers: TransitionalProviderDraftsSchema,
    credentialEpochs: z.partialRecord(
      ProviderIdSchema,
      z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
    ),
  })
  .strict();
const Task7PrivacySettingsSchema = z
  .object({
    historyEnabled: z.boolean(),
    historyRetentionDays: z.union([z.literal(7), z.literal(30), z.literal(90)]).nullable(),
  })
  .strict();

const LegacyVoiceCommandSchema = z
  .object({
    id: z.uuid(),
    trigger: z.string().trim().min(1).max(VOICE_TRIGGER_MAX_LENGTH),
    snippet: z.string().min(1).max(VOICE_SNIPPET_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyVoiceCommandListSchema = z.array(LegacyVoiceCommandSchema).max(VOICE_COMMAND_LIMIT);
const LegacyVocabularyEntrySchema = z
  .object({
    id: z.uuid(),
    value: z.string().trim().min(1).max(VOCABULARY_VALUE_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyVocabularyListSchema = z.array(LegacyVocabularyEntrySchema).max(VOCABULARY_LIMIT);

const LegacySettingsV1Schema = z
  .object({ schemaVersion: z.literal(1), app: LegacyAppSettingsSchema })
  .strict();
const Task4SettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    recording: RecordingSettingsSchema,
  })
  .strict();
const Task9SettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
  })
  .strict();
const CombinedSettingsV2Schema = z
  .object({
    schemaVersion: z.literal(2),
    app: LegacyAppSettingsSchema,
    recording: RecordingSettingsSchema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
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
    recording: RecordingSettingsSchema,
    smartProcessing: TransitionalSmartProcessingSettingsSchema,
  })
  .strict();

// Both completed topics independently emitted schema version 4. Keep each historical
// document shape explicit, plus the valid hybrid shape produced by defensive integrations.
const Task6SettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    smartProcessing: LegacySmartProcessingSettingsSchema,
  })
  .strict();
const Task11SettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: LegacyAppSettingsSchema,
    recording: RecordingSettingsSchema,
    smartProcessing: TransitionalSmartProcessingSettingsSchema,
  })
  .strict();
const HybridSettingsV4Schema = z
  .object({
    schemaVersion: z.literal(4),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    smartProcessing: TransitionalSmartProcessingSettingsSchema,
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
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    smartProcessing: TransitionalSmartProcessingSettingsSchema,
  })
  .strict();
const LegacySettingsV6Schema = z
  .object({
    schemaVersion: z.literal(6),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    smartProcessing: Task9SmartProcessingSettingsSchema,
  })
  .strict();

const Task7SettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    privacy: Task7PrivacySettingsSchema,
    smartProcessing: Task9SmartProcessingSettingsSchema,
  })
  .strict();
const Task8SettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    smartProcessing: Task9SmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
  })
  .strict();
const HybridSettingsV7Schema = z
  .object({
    schemaVersion: z.literal(7),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    privacy: Task7PrivacySettingsSchema,
    smartProcessing: Task9SmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
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
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    privacy: Task7PrivacySettingsSchema,
    smartProcessing: Task9SmartProcessingSettingsSchema,
    voiceCommands: LegacyVoiceCommandListSchema,
    customVocabulary: LegacyVocabularyListSchema,
  })
  .strict();
const LegacySettingsV9Schema = LegacySettingsV8Schema.extend({ schemaVersion: z.literal(9) });
const LegacySettingsV10Schema = z
  .object({
    schemaVersion: z.literal(10),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    privacy: Task7PrivacySettingsSchema.extend({ retainSmartScreenshots: z.boolean() }),
    smartProcessing: Task9SmartProcessingSettingsSchema.extend({
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
    }),
    voiceCommands: VoiceCommandListSchema,
    customVocabulary: VocabularyListSchema,
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
const PrePiPathSmartProcessingSettingsSchema = SmartProcessingSettingsSchema.omit({
  piInstallationPath: true,
});
const LegacySettingsV13Schema = z
  .object({
    schemaVersion: z.literal(13),
    app: CurrentAppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    privacy: PrivacySettingsSchema.omit({ diagnosticLoggingEnabled: true }),
    smartProcessing: PrePiPathSmartProcessingSettingsSchema,
    voiceCommands: VoiceCommandListSchema,
    customVocabulary: VocabularyListSchema,
    welcome: LegacyWelcomeSettingsSchema,
  })
  .strict();

const LegacyProviderIdV14Schema = ProviderIdSchema.exclude(['pi']);
const LegacyProviderDraftsV14Schema = z
  .partialRecord(LegacyProviderIdV14Schema, ProviderSettingsDraftSchema)
  .superRefine(validateLegacyProviderDrafts);
const LegacySmartProcessingV14Schema = PrePiPathSmartProcessingSettingsSchema.extend({
  selectedProviderId: LegacyProviderIdV14Schema,
  providers: LegacyProviderDraftsV14Schema,
});
const LegacySettingsV14Schema = SettingsObjectSchema.omit({
  app: true,
  transcription: true,
  dictationProfiles: true,
  smartProcessing: true,
  welcome: true,
}).extend({
  schemaVersion: z.literal(14),
  app: AppSettingsSchema,
  transcription: LegacyTranscriptionSettingsSchema,
  smartProcessing: LegacySmartProcessingV14Schema,
  welcome: LegacyWelcomeSettingsSchema,
});
const LegacySettingsV15Schema = SettingsObjectSchema.omit({
  app: true,
  transcription: true,
  dictationProfiles: true,
  smartProcessing: true,
  welcome: true,
}).extend({
  schemaVersion: z.literal(15),
  app: AppSettingsSchema,
  transcription: LegacyTranscriptionSettingsSchema,
  smartProcessing: PrePiPathSmartProcessingSettingsSchema,
  welcome: LegacyWelcomeSettingsSchema,
});
const LegacySettingsV17Schema = SettingsObjectSchema.omit({
  app: true,
  transcription: true,
  dictationProfiles: true,
  smartProcessing: true,
  welcome: true,
}).extend({
  schemaVersion: z.literal(17),
  app: AppSettingsSchema,
  transcription: LegacyTranscriptionSettingsSchema,
  smartProcessing: SmartProcessingSettingsSchema.extend({ piExtensionsEnabled: z.boolean() }),
  welcome: LegacyWelcomeSettingsSchema,
});
const LegacySettingsV18Schema = SettingsObjectSchema.omit({
  app: true,
  transcription: true,
  dictationProfiles: true,
  welcome: true,
}).extend({
  schemaVersion: z.literal(18),
  app: AppSettingsSchema,
  transcription: LegacyTranscriptionSettingsSchema,
  welcome: LegacyWelcomeSettingsSchema,
});

interface LegacyBase {
  readonly app: z.infer<typeof LegacyAppSettingsSchema> | z.infer<typeof AppSettingsSchema>;
  readonly recording?: z.infer<typeof RecordingSettingsSchema>;
  readonly transcription?: z.infer<typeof LegacyTranscriptionSettingsSchema>;
  readonly privacy?: z.infer<typeof Task7PrivacySettingsSchema>;
  readonly voiceCommands?: readonly VoiceCommand[];
  readonly customVocabulary?: readonly VocabularyEntry[];
  readonly smartProcessing?: {
    readonly selectedProviderId: ProviderId;
    readonly providers: Partial<Record<ProviderId, ProviderSettingsDraft>>;
    readonly credentialEpochs?: Partial<Record<ProviderId, number>>;
  };
}

export const SETTINGS_MIGRATIONS: SettingsMigrations = Object.freeze({
  1: (input) =>
    migrateLegacy(LegacySettingsV1Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  2: (input) =>
    migrateLegacy(LegacySettingsV2Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  3: (input) =>
    migrateLegacy(LegacySettingsV3Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  4: (input) =>
    migrateLegacy(LegacySettingsV4Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  5: (input) =>
    migrateLegacy(LegacySettingsV5Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  6: (input) =>
    migrateLegacy(LegacySettingsV6Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  7: (input) =>
    migrateLegacy(LegacySettingsV7Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  8: (input) =>
    migrateLegacy(LegacySettingsV8Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  9: (input) =>
    migrateLegacy(LegacySettingsV9Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  10: (input) =>
    migrateLegacy(LegacySettingsV10Schema.parse(stripTask12Fields(stripDictationProfiles(input)))),
  11: (input) =>
    migrateUnverifiedWelcome(
      LegacySettingsV11Schema.parse(stripDiagnosticLoggingField(stripDictationProfiles(input))),
    ),
  12: (input) =>
    migrateUnverifiedWelcome(
      LegacySettingsV12Schema.parse(stripDiagnosticLoggingField(stripDictationProfiles(input))),
    ),
  13: (input) => {
    const legacy = LegacySettingsV13Schema.parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        privacy: { ...legacy.privacy, diagnosticLoggingEnabled: false },
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  14: (input) => {
    const legacy = LegacySettingsV14Schema.parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  15: (input) => {
    const legacy = LegacySettingsV15Schema.parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  16: (input) => {
    const legacy = structuredClone(stripDictationProfiles(input)) as Record<string, unknown>;
    const smartProcessing = legacy.smartProcessing;
    if (
      typeof smartProcessing !== 'object' ||
      smartProcessing === null ||
      Array.isArray(smartProcessing)
    )
      throw new Error('Invalid v16 Smart settings');
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing,
      }),
    );
  },
  17: (input) => {
    const legacy = LegacySettingsV17Schema.parse(stripDictationProfiles(input));
    const { piExtensionsEnabled, ...smartProcessing } = legacy.smartProcessing;
    void piExtensionsEnabled;
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing,
      }),
    );
  },
  18: (input) => {
    const legacy = LegacySettingsV18Schema.parse(stripDictationProfiles(input));
    return SettingsSchema.parse(
      migrateRemovedLargeModel({ ...legacy, schemaVersion: SETTINGS_SCHEMA_VERSION }),
    );
  },
});

function migrateRemovedLargeModel(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const migrated = structuredClone(input) as Record<string, unknown>;
  const app = migrated.app;
  if (typeof app === 'object' && app !== null && !Array.isArray(app)) {
    const legacyApp = app as Record<string, unknown>;
    const profiles = defaultDictationProfiles();
    const general = profiles[0];
    if (general === undefined) throw new Error('Default General profile is missing');
    profiles[0] = {
      ...general,
      activationKey: ActivationKeySchema.parse(legacyApp.activationKey),
      processingMode: ProcessingModeSchema.parse(legacyApp.defaultProcessingMode),
    };
    migrated.dictationProfiles = profiles;
  }
  const transcription = migrated.transcription;
  if (
    typeof transcription === 'object' &&
    transcription !== null &&
    !Array.isArray(transcription) &&
    (transcription as Record<string, unknown>).modelId === 'Xenova/whisper-large'
  ) {
    (transcription as Record<string, unknown>).modelId = 'onnx-community/whisper-large-v3-turbo';
  }
  const welcome = migrated.welcome;
  if (typeof welcome === 'object' && welcome !== null && !Array.isArray(welcome)) {
    // Earlier evidence omitted profile identity and Shift, so it is not proof
    // of any exact v19 activation binding.
    (welcome as Record<string, unknown>).activationEvidence = null;
    (welcome as Record<string, unknown>).activationTested = false;
    const modelEvidence = (welcome as Record<string, unknown>).modelEvidence;
    if (
      typeof modelEvidence === 'object' &&
      modelEvidence !== null &&
      !Array.isArray(modelEvidence) &&
      (modelEvidence as Record<string, unknown>).modelId === 'Xenova/whisper-large'
    ) {
      // The old model's revision cannot prove that the replacement model is installed.
      (welcome as Record<string, unknown>).modelEvidence = null;
    }
  }
  return migrated;
}

function stripDictationProfiles(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const clone = structuredClone(input) as Record<string, unknown>;
  delete clone.dictationProfiles;
  return clone;
}

function stripTask12Fields(input: unknown): unknown {
  const stripped = stripDiagnosticLoggingField(stripDictationProfiles(input));
  if (typeof stripped !== 'object' || stripped === null || Array.isArray(stripped)) return stripped;
  const clone = structuredClone(stripped) as Record<string, unknown>;
  delete clone.welcome;
  if (typeof clone.app === 'object' && clone.app !== null && !Array.isArray(clone.app)) {
    delete (clone.app as Record<string, unknown>).launchAtLogin;
  }
  return clone;
}

function stripPiInstallationPathOnly(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const clone = structuredClone(input) as Record<string, unknown>;
  if (
    typeof clone.smartProcessing === 'object' &&
    clone.smartProcessing !== null &&
    !Array.isArray(clone.smartProcessing)
  ) {
    delete (clone.smartProcessing as Record<string, unknown>).piInstallationPath;
  }
  return clone;
}

function stripPiInstallationPath(input: unknown): unknown {
  const clone = stripPiInstallationPathOnly(input);
  if (typeof clone !== 'object' || clone === null || Array.isArray(clone)) return clone;
  const record = clone as Record<string, unknown>;
  if (
    typeof record.smartProcessing === 'object' &&
    record.smartProcessing !== null &&
    !Array.isArray(record.smartProcessing)
  ) {
    delete (record.smartProcessing as Record<string, unknown>).piExtensionsEnabled;
  }
  return record;
}

function stripDiagnosticLoggingField(input: unknown): unknown {
  const withoutPiPath = stripPiInstallationPath(stripDictationProfiles(input));
  if (typeof withoutPiPath !== 'object' || withoutPiPath === null || Array.isArray(withoutPiPath))
    return withoutPiPath;
  const clone = structuredClone(withoutPiPath) as Record<string, unknown>;
  if (
    typeof clone.privacy === 'object' &&
    clone.privacy !== null &&
    !Array.isArray(clone.privacy)
  ) {
    delete (clone.privacy as Record<string, unknown>).diagnosticLoggingEnabled;
  }
  return clone;
}

function migrateUnverifiedWelcome(
  legacy: z.infer<typeof LegacySettingsV11Schema> | z.infer<typeof LegacySettingsV12Schema>,
): Settings {
  const migrated = migrateLegacy(legacy);
  // Completion is durable historical UI state, while partial progress still resumes at the
  // earliest evidence-bearing step without manufacturing microphone/model/helper proof.
  const completedAt = legacy.welcome.completedAt;
  migrated.welcome = {
    ...structuredClone(DEFAULT_SETTINGS.welcome),
    lastStep: completedAt === null ? (legacy.welcome.lastStep === 1 ? 1 : 2) : 6,
    completedAt,
  };
  return migrated;
}

function migrateLegacy(legacy: LegacyBase): Settings {
  const legacyApp = legacy.app as Record<string, unknown>;
  const profiles = defaultDictationProfiles();
  if (legacyApp.activationKey !== undefined) {
    const general = profiles[0];
    if (general === undefined) throw new Error('Default General profile is missing');
    profiles[0] = {
      ...general,
      activationKey: ActivationKeySchema.parse(legacyApp.activationKey),
      processingMode: ProcessingModeSchema.parse(legacyApp.defaultProcessingMode),
    };
  }
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    app: {
      ...structuredClone(DEFAULT_SETTINGS.app),
      enabled: legacy.app.enabled,
      closeToTray: legacy.app.closeToTray,
      activationKey:
        'activationKey' in legacy.app
          ? legacy.app.activationKey
          : DEFAULT_SETTINGS.app.activationKey,
      defaultProcessingMode:
        'defaultProcessingMode' in legacy.app
          ? legacy.app.defaultProcessingMode
          : DEFAULT_SETTINGS.app.defaultProcessingMode,
      ...('widgetSize' in legacy.app ? { widgetSize: legacy.app.widgetSize } : {}),
      ...('soundsEnabled' in legacy.app ? { soundsEnabled: legacy.app.soundsEnabled } : {}),
      launchAtLogin:
        'launchAtLogin' in legacy.app && legacy.app.launchAtLogin !== undefined
          ? legacy.app.launchAtLogin
          : DEFAULT_SETTINGS.app.launchAtLogin,
    },
    recording: legacy.recording ?? structuredClone(DEFAULT_SETTINGS.recording),
    transcription: migrateLegacyTranscription(legacy.transcription),
    dictationProfiles: profiles,
    privacy: {
      ...structuredClone(DEFAULT_SETTINGS.privacy),
      ...(legacy.privacy ?? {}),
    },
    smartProcessing:
      legacy.smartProcessing === undefined
        ? structuredClone(DEFAULT_SETTINGS.smartProcessing)
        : migrateLegacySmartProcessing(legacy.smartProcessing),
    voiceCommands: retainBoundedCommands(
      structuredClone(legacy.voiceCommands ?? DEFAULT_SETTINGS.voiceCommands),
    ),
    customVocabulary: retainBoundedVocabulary(
      structuredClone(legacy.customVocabulary ?? DEFAULT_SETTINGS.customVocabulary),
    ),
    welcome: structuredClone(DEFAULT_SETTINGS.welcome),
  };
}

function migrateLegacyTranscription(
  legacy: LegacyBase['transcription'],
): Settings['transcription'] {
  if (legacy === undefined) return structuredClone(DEFAULT_SETTINGS.transcription);
  return {
    modelId:
      legacy.modelId === 'Xenova/whisper-large'
        ? 'onnx-community/whisper-large-v3-turbo'
        : legacy.modelId,
    language: legacy.language,
  };
}

function migrateLegacySmartProcessing(
  legacy: NonNullable<LegacyBase['smartProcessing']>,
): Settings['smartProcessing'] {
  const providers = structuredClone(legacy.providers);
  const bedrock = providers.bedrock;
  if (bedrock !== undefined && bedrock.region === undefined) {
    providers.bedrock = { ...bedrock, region: 'us-west-2' };
  }
  return {
    selectedProviderId: legacy.selectedProviderId,
    providers,
    credentialEpochs: structuredClone(legacy.credentialEpochs ?? {}),
    piInstallationPath: null,
    onScreenAwarenessEnabled: false,
    visionOverrides: [],
  };
}

function retainBoundedCommands(commands: readonly VoiceCommand[]): VoiceCommand[] {
  const retained: VoiceCommand[] = [];
  let bytes = 0;
  for (const command of commands) {
    const parsed = VoiceCommandSchema.safeParse(command);
    if (!parsed.success) continue;
    const nextBytes =
      bytes + utf8ByteLength(parsed.data.trigger) + utf8ByteLength(parsed.data.snippet);
    if (nextBytes > VOICE_COMMANDS_MAX_UTF8_BYTES) continue;
    retained.push(parsed.data);
    bytes = nextBytes;
  }
  return VoiceCommandListSchema.parse(retained);
}

function retainBoundedVocabulary(entries: readonly VocabularyEntry[]): VocabularyEntry[] {
  const retained: VocabularyEntry[] = [];
  let bytes = 0;
  for (const entry of entries) {
    const parsed = VocabularyEntrySchema.safeParse(entry);
    if (!parsed.success) continue;
    const nextBytes = bytes + utf8ByteLength(parsed.data.value);
    if (nextBytes > VOCABULARY_TOTAL_MAX_UTF8_BYTES) continue;
    retained.push(parsed.data);
    bytes = nextBytes;
  }
  return VocabularyListSchema.parse(retained);
}

function validateLegacyProviderDrafts(
  drafts: Partial<Record<ProviderId, ProviderSettingsDraft>>,
  context: z.core.$RefinementCtx,
): void {
  for (const [providerId, draft] of Object.entries(drafts)) {
    const parsedId = ProviderIdSchema.safeParse(providerId);
    if (!parsedId.success) continue;
    const candidate =
      parsedId.data === 'bedrock' && draft.region === undefined
        ? { providerId: parsedId.data, ...draft, region: 'us-west-2' }
        : { providerId: parsedId.data, ...draft };
    const parsed = ProviderConfigSchema.safeParse(candidate);
    if (parsed.success) continue;
    for (const issue of parsed.error.issues) {
      context.addIssue({
        code: 'custom',
        path: [providerId, ...issue.path],
        message: issue.message,
      });
    }
  }
}
