import { z } from 'zod';
import { WelcomeSettingsSchema } from './welcome';
import { ActivationKeySchema } from '../helper/protocol';
import { VoiceCommandListSchema } from './commands';
import { VocabularyListSchema } from './vocabulary';
import { MicrophoneIdSchema, SilencePresetSchema } from './audio';
import { ProcessingModeSchema } from './history';
import { WhisperModelIdSchema } from './model-manifest';
import { DictationProfileListSchema, defaultDictationProfiles } from './dictation-profiles';
import {
  AwsRegionSchema,
  AzureModelTypeSchema,
  PersistedProviderBaseUrlSchema,
  PersistedProviderConfigSchema,
  ProviderBaseUrlSchema,
  ProviderIdSchema,
  ProviderModelIdSchema,
  PiThinkingLevelSchema,
} from './providers';
import { TranscriptionLanguageSchema } from './transcription';

export const SETTINGS_SCHEMA_VERSION = 19 as const;

export const PiInstallationPathSchema = z.string().trim().min(1).max(8_192).nullable();

export const HistoryRetentionDaysSchema = z.union([z.literal(7), z.literal(30), z.literal(90)]);

export const PrivacySettingsSchema = z
  .object({
    historyEnabled: z.boolean(),
    historyRetentionDays: HistoryRetentionDaysSchema.nullable(),
    retainSmartScreenshots: z.boolean(),
    diagnosticLoggingEnabled: z.boolean(),
  })
  .strict();

export const WidgetSizeSchema = z.enum(['default', 'large', 'huge', 'max']);

export const AppSettingsSchema = z
  .object({
    enabled: z.boolean(),
    closeToTray: z.boolean(),
    // Retained as a compatibility mirror for pre-profile clients; profiles are authoritative.
    activationKey: ActivationKeySchema,
    defaultProcessingMode: ProcessingModeSchema,
    widgetSize: WidgetSizeSchema,
    soundsEnabled: z.boolean(),
    launchAtLogin: z.boolean(),
  })
  .strict();

export const RecordingSettingsSchema = z
  .object({
    preferredMicrophoneId: MicrophoneIdSchema.nullable(),
    silencePreset: SilencePresetSchema,
  })
  .strict();

export const TranscriptionSettingsSchema = z
  .object({
    modelId: WhisperModelIdSchema,
    language: TranscriptionLanguageSchema.nullable(),
  })
  .strict();

const ProviderDraftFieldsSchema = z
  .object({
    baseUrl: PersistedProviderBaseUrlSchema.optional(),
    modelId: ProviderModelIdSchema.nullable().optional(),
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
    region: AwsRegionSchema.optional(),
    modelType: AzureModelTypeSchema.optional(),
    thinking: PiThinkingLevelSchema.optional(),
  })
  .strict();

export const ProviderSettingsDraftSchema = ProviderDraftFieldsSchema;
export type ProviderSettingsDraft = z.infer<typeof ProviderSettingsDraftSchema>;

const ProviderSettingsMutationSchema = ProviderDraftFieldsSchema.extend({
  baseUrl: ProviderBaseUrlSchema.optional(),
});
const ProviderDraftsSchema = z
  .partialRecord(ProviderIdSchema, ProviderSettingsDraftSchema)
  .superRefine(validateProviderDrafts);

export const CredentialEpochsSchema = z.partialRecord(
  ProviderIdSchema,
  z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
);

export const VisionOverrideSchema = z
  .object({
    providerId: z.enum(['generic-openai', 'litellm']),
    binding: z.string().min(1).max(2_048),
    modelId: ProviderModelIdSchema,
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();

export const SmartProcessingSettingsSchema = z
  .object({
    selectedProviderId: ProviderIdSchema,
    providers: ProviderDraftsSchema,
    credentialEpochs: CredentialEpochsSchema,
    piInstallationPath: PiInstallationPathSchema,
    onScreenAwarenessEnabled: z.boolean(),
    visionOverrides: z.array(VisionOverrideSchema).max(64),
  })
  .strict();

export const SettingsObjectSchema = z
  .object({
    schemaVersion: z.literal(SETTINGS_SCHEMA_VERSION),
    app: AppSettingsSchema,
    recording: RecordingSettingsSchema,
    transcription: TranscriptionSettingsSchema,
    dictationProfiles: DictationProfileListSchema,
    privacy: PrivacySettingsSchema,
    smartProcessing: SmartProcessingSettingsSchema,
    voiceCommands: VoiceCommandListSchema,
    customVocabulary: VocabularyListSchema,
    welcome: WelcomeSettingsSchema,
  })
  .strict();

export const SettingsSchema = SettingsObjectSchema.superRefine((settings, context) => {
  const general = settings.dictationProfiles.find((profile) => profile.id === 'general');
  if (general === undefined) return;
  if (settings.app.activationKey !== general.activationKey) {
    context.addIssue({
      code: 'custom',
      path: ['app', 'activationKey'],
      message: 'The activation key compatibility mirror must match the General profile.',
    });
  }
  if (settings.app.defaultProcessingMode !== general.processingMode) {
    context.addIssue({
      code: 'custom',
      path: ['app', 'defaultProcessingMode'],
      message: 'The processing mode compatibility mirror must match the General profile.',
    });
  }
});

const AppSettingsPatchSchema = AppSettingsSchema.partial();
const PublicAppSettingsPatchSchema = AppSettingsSchema.omit({
  activationKey: true,
  defaultProcessingMode: true,
}).partial();
const RecordingSettingsPatchSchema = RecordingSettingsSchema.partial();
const TranscriptionSettingsPatchSchema = TranscriptionSettingsSchema.partial();
const PrivacySettingsPatchSchema = PrivacySettingsSchema.partial();

export const SettingsPatchSchema = z
  .object({
    app: AppSettingsPatchSchema.optional(),
    recording: RecordingSettingsPatchSchema.optional(),
    transcription: TranscriptionSettingsPatchSchema.optional(),
    dictationProfiles: DictationProfileListSchema.optional(),
    privacy: PrivacySettingsPatchSchema.optional(),
    smartProcessing: z
      .object({
        selectedProviderId: ProviderIdSchema.optional(),
        providers: z.partialRecord(ProviderIdSchema, ProviderSettingsMutationSchema).optional(),
        providerReplacements: z
          .partialRecord(ProviderIdSchema, ProviderSettingsMutationSchema)
          .optional(),
        credentialEpochs: CredentialEpochsSchema.optional(),
        piInstallationPath: PiInstallationPathSchema.optional(),
        onScreenAwarenessEnabled: z.boolean().optional(),
        visionOverrides: z.array(VisionOverrideSchema).max(64).optional(),
      })
      .strict()
      .optional(),
    voiceCommands: VoiceCommandListSchema.optional(),
    customVocabulary: VocabularyListSchema.optional(),
    welcome: WelcomeSettingsSchema.partial().optional(),
  })
  .strict()
  .refine(hasDefinedLeaf, { message: 'A settings patch must contain at least one setting' });

export const PublicSettingsPatchSchema = z
  .object({
    app: PublicAppSettingsPatchSchema.optional(),
    recording: RecordingSettingsPatchSchema.optional(),
    transcription: TranscriptionSettingsPatchSchema.optional(),
    privacy: PrivacySettingsPatchSchema.optional(),
  })
  .strict()
  .refine(hasDefinedLeaf, { message: 'A settings patch must contain at least one setting' });

export type Settings = z.infer<typeof SettingsSchema>;
export type SettingsPatch = z.infer<typeof SettingsPatchSchema>;
export type PublicSettingsPatch = z.infer<typeof PublicSettingsPatchSchema>;

export const DEFAULT_SETTINGS: Settings = deepFreeze({
  schemaVersion: SETTINGS_SCHEMA_VERSION,
  app: {
    enabled: true,
    closeToTray: true,
    activationKey: 'Z',
    defaultProcessingMode: 'raw',
    widgetSize: 'default',
    soundsEnabled: true,
    launchAtLogin: false,
  },
  recording: {
    preferredMicrophoneId: null,
    silencePreset: 'average',
  },
  transcription: {
    modelId: 'onnx-community/whisper-large-v3-turbo',
    language: null,
  },
  dictationProfiles: defaultDictationProfiles(),
  privacy: {
    historyEnabled: true,
    historyRetentionDays: null,
    retainSmartScreenshots: false,
    diagnosticLoggingEnabled: false,
  },
  voiceCommands: [],
  customVocabulary: [],
  welcome: {
    completedAt: null,
    lastStep: 1,
    microphoneTested: false,
    activationTested: false,
    microphoneEvidence: null,
    activationEvidence: null,
    modelEvidence: null,
    revision: 0,
  },
  smartProcessing: {
    selectedProviderId: 'ollama',
    providers: {
      ollama: {
        baseUrl: 'http://127.0.0.1:11434',
        keepAlive: 300,
      },
    },
    credentialEpochs: {},
    piInstallationPath: null,
    onScreenAwarenessEnabled: false,
    visionOverrides: [],
  },
});

function validateProviderDrafts(
  drafts: Partial<Record<z.infer<typeof ProviderIdSchema>, ProviderSettingsDraft>>,
  context: z.core.$RefinementCtx,
): void {
  for (const [providerId, draft] of Object.entries(drafts)) {
    const parsedId = ProviderIdSchema.safeParse(providerId);
    if (!parsedId.success) continue;
    const parsed = PersistedProviderConfigSchema.safeParse({
      providerId: parsedId.data,
      ...draft,
    });
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

function hasDefinedLeaf(value: unknown): boolean {
  if (value === undefined) return false;
  if (value === null || Array.isArray(value) || typeof value !== 'object') return true;
  return Object.entries(value).some(([key, nested]) => {
    if (key === 'providerReplacements' && isObject(nested)) {
      return Object.keys(nested).length > 0;
    }
    return hasDefinedLeaf(nested);
  });
}

function isObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function deepFreeze<Value>(value: Value): Value {
  if (value !== null && typeof value === 'object' && !Object.isFrozen(value)) {
    Object.freeze(value);
    for (const nested of Object.values(value)) deepFreeze(nested);
  }
  return value;
}
