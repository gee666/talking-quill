import { z } from 'zod';

// These schemas are snapshots of released on-disk settings contracts. Do not replace their
// literals, bounds, or refinements with imports from mutable current schemas.
export const LegacyActivationKeySchema = z.enum([
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'O',
  'P',
  'Q',
  'R',
  'S',
  'T',
  'U',
  'V',
  'W',
  'X',
  'Y',
  'Z',
]);
export const LegacyProcessingModeSchema = z.enum(['raw', 'smart']);
const LegacyWidgetSizeSchema = z.enum(['default', 'large', 'huge', 'max']);

export const LegacyAppSettingsSchema = z
  .object({ enabled: z.boolean(), closeToTray: z.boolean() })
  .strict();
export const LegacyExtendedAppSettingsSchema = z
  .object({
    enabled: z.boolean(),
    closeToTray: z.boolean(),
    activationKey: LegacyActivationKeySchema,
    defaultProcessingMode: LegacyProcessingModeSchema,
    widgetSize: LegacyWidgetSizeSchema,
    soundsEnabled: z.boolean(),
    launchAtLogin: z.boolean().optional(),
  })
  .strict();
export const LegacyRequiredLaunchAppSettingsSchema = LegacyExtendedAppSettingsSchema.extend({
  launchAtLogin: z.boolean(),
});

const LegacyMicrophoneIdSchema = z.string().min(1).max(1_024);
export const LegacyRecordingSettingsSchema = z
  .object({
    preferredMicrophoneId: LegacyMicrophoneIdSchema.nullable(),
    silencePreset: z.enum(['aggressive', 'average', 'relaxed']),
  })
  .strict();

export const LegacyWhisperModelIdSchema = z.enum([
  'Xenova/whisper-small',
  'Xenova/whisper-large',
  'onnx-community/whisper-large-v3-turbo',
]);
export const LegacyTranscriptionSettingsSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    language: z.string().trim().min(1).max(80).nullable(),
  })
  .strict();

export const LegacyProviderIdSchema = z.enum([
  'openai',
  'generic-openai',
  'lmstudio',
  'localai',
  'koboldcpp',
  'textgenwebui',
  'docker-model-runner',
  'lemonade',
  'foundry',
  'omlx',
  'groq',
  'openrouter',
  'togetherai',
  'fireworksai',
  'deepseek',
  'perplexity',
  'mistral',
  'novita',
  'cometapi',
  'ppio',
  'apipie',
  'sambanova',
  'cerebras',
  'giteeai',
  'minimax',
  'moonshotai',
  'zai',
  'xai',
  'nvidia-nim',
  'privatemode',
  'litellm',
  'ollama',
  'pi',
  'anthropic',
  'gemini',
  'azure',
  'bedrock',
  'cohere',
]);
export const LegacyProviderIdV14Schema = LegacyProviderIdSchema.exclude(['pi']);
export type LegacyProviderId = z.infer<typeof LegacyProviderIdSchema>;

const legacyConfigurableProviderIds = new Set<LegacyProviderId>([
  'generic-openai',
  'lmstudio',
  'localai',
  'koboldcpp',
  'textgenwebui',
  'docker-model-runner',
  'lemonade',
  'foundry',
  'omlx',
  'nvidia-nim',
  'privatemode',
  'litellm',
  'ollama',
  'azure',
]);
const legacyTimeoutProviderIds = new Set<LegacyProviderId>(['openrouter', 'novita', 'cometapi']);

// Released settings accepted any URL protocol. Runtime and mutation validation intentionally use
// a stricter current contract, leaving file:/ftp: values persisted but inert until repaired.
const LegacyProviderBaseUrlSchema = z
  .url()
  .max(2_048)
  .superRefine((value, context) => {
    if (!URL.canParse(value)) return;
    const endpoint = new URL(value);
    if (
      endpoint.username.length > 0 ||
      endpoint.password.length > 0 ||
      endpoint.search.length > 0 ||
      endpoint.hash.length > 0
    ) {
      context.addIssue({
        code: 'custom',
        message: 'Provider endpoints cannot contain credentials, queries, or fragments.',
      });
    }
  });
const LegacyProviderModelIdSchema = z.string().trim().min(1).max(512);
const LegacyAwsRegionSchema = z
  .string()
  .regex(
    /^(?:af|ap|ca|eu|il|me|mx|sa|us)-(?:central|east|north|northeast|northwest|south|southeast|southwest|west)-\d$/,
  )
  .max(32);
const LegacyAzureModelTypeSchema = z.enum(['default', 'reasoning']);
const LegacyPiThinkingLevelSchema = z.enum([
  'off',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]);
const LegacyKeepAliveSchema = z.union([
  z.number().int().min(-1).max(86_400),
  z
    .string()
    .trim()
    .min(1)
    .max(32)
    .regex(/^(?:\d+(?:\.\d+)?(?:ns|us|µs|ms|s|m|h))+$/u),
]);

export const LegacyProviderDraftV2Schema = z
  .object({
    baseUrl: LegacyProviderBaseUrlSchema.optional(),
    modelId: LegacyProviderModelIdSchema.nullable().optional(),
    contextWindow: z.number().int().min(1).max(2_000_000).optional(),
    maxOutputTokens: z.number().int().min(1).max(16_384).optional(),
    timeoutMs: z.number().int().min(500).max(120_000).optional(),
    keepAlive: LegacyKeepAliveSchema.optional(),
  })
  .strict();
export const LegacyProviderDraftSchema = LegacyProviderDraftV2Schema.extend({
  region: LegacyAwsRegionSchema.optional(),
  modelType: LegacyAzureModelTypeSchema.optional(),
  thinking: LegacyPiThinkingLevelSchema.optional(),
});
export type LegacyProviderDraft = z.infer<typeof LegacyProviderDraftSchema>;

export const LegacyProviderDraftsV2Schema = z
  .partialRecord(LegacyProviderIdSchema, LegacyProviderDraftV2Schema)
  .superRefine(validateLegacyProviderDrafts);
export const LegacyProviderDraftsSchema = z
  .partialRecord(LegacyProviderIdSchema, LegacyProviderDraftSchema)
  .superRefine(validateLegacyProviderDrafts);
export const LegacyProviderDraftsV14Schema = z
  .partialRecord(LegacyProviderIdV14Schema, LegacyProviderDraftSchema)
  .superRefine(validateLegacyProviderDrafts);

export const LegacyCredentialEpochsSchema = z.partialRecord(
  LegacyProviderIdSchema,
  z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
);
const LegacyVisionOverrideSchema = z
  .object({
    providerId: z.enum(['generic-openai', 'litellm']),
    binding: z.string().min(1).max(2_048),
    modelId: LegacyProviderModelIdSchema,
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyVisionOverridesSchema = z.array(LegacyVisionOverrideSchema).max(64);

export const LegacySmartProcessingPrePiPathSchema = z
  .object({
    selectedProviderId: LegacyProviderIdSchema,
    providers: LegacyProviderDraftsSchema,
    credentialEpochs: LegacyCredentialEpochsSchema,
    onScreenAwarenessEnabled: z.boolean(),
    visionOverrides: LegacyVisionOverridesSchema,
  })
  .strict();
export const LegacySmartProcessingSettingsSchema = LegacySmartProcessingPrePiPathSchema.extend({
  piInstallationPath: z.string().trim().min(1).max(8_192).nullable(),
});
export const LegacySmartProcessingV14Schema = LegacySmartProcessingPrePiPathSchema.extend({
  selectedProviderId: LegacyProviderIdV14Schema,
  providers: LegacyProviderDraftsV14Schema,
});

export const LegacyPrivacyV7Schema = z
  .object({
    historyEnabled: z.boolean(),
    historyRetentionDays: z.union([z.literal(7), z.literal(30), z.literal(90)]).nullable(),
  })
  .strict();
export const LegacyPrivacyV10Schema = LegacyPrivacyV7Schema.extend({
  retainSmartScreenshots: z.boolean(),
});
export const LegacyPrivacyV14Schema = LegacyPrivacyV10Schema.extend({
  diagnosticLoggingEnabled: z.boolean(),
});

const LEGACY_VOICE_COMMAND_LIMIT = 100;
const LEGACY_VOICE_TRIGGER_MAX_LENGTH = 200;
const LEGACY_VOICE_TRIGGER_MAX_UTF8_BYTES = 400;
const LEGACY_VOICE_SNIPPET_MAX_LENGTH = 100_000;
const LEGACY_VOICE_SNIPPET_MAX_UTF8_BYTES = 200_000;
const LEGACY_VOICE_COMMANDS_MAX_UTF8_BYTES = 512_000;

export const LegacyLooseVoiceCommandSchema = z
  .object({
    id: z.uuid(),
    trigger: z.string().trim().min(1).max(LEGACY_VOICE_TRIGGER_MAX_LENGTH),
    snippet: z.string().min(1).max(LEGACY_VOICE_SNIPPET_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyLooseVoiceCommandListSchema = z
  .array(LegacyLooseVoiceCommandSchema)
  .max(LEGACY_VOICE_COMMAND_LIMIT);

const LegacyVoiceCommandTriggerSchema = z
  .string()
  .trim()
  .min(1)
  .max(LEGACY_VOICE_TRIGGER_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOICE_TRIGGER_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOICE_TRIGGER_MAX_UTF8_BYTES,
    'Trigger is too large when encoded as UTF-8',
  )
  .refine(
    (value) => !hasLegacyControlCharacters(value, false),
    'Control characters are not allowed',
  )
  .refine(
    (value) => /[\p{L}\p{N}]/u.test(normalizeLegacyCommandText(value)),
    'Trigger must contain a letter or number after normalization',
  );
const LegacyVoiceCommandSnippetSchema = z
  .string()
  .min(1)
  .max(LEGACY_VOICE_SNIPPET_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOICE_SNIPPET_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOICE_SNIPPET_MAX_UTF8_BYTES,
    'Snippet is too large when encoded as UTF-8',
  )
  .refine((value) => value.trim().length > 0, 'Snippet must contain text')
  .refine(
    (value) => !hasLegacyControlCharacters(value, true),
    'Unsupported control characters are not allowed',
  );
export const LegacyVoiceCommandSchema = z
  .object({
    id: z.uuid(),
    trigger: LegacyVoiceCommandTriggerSchema,
    snippet: LegacyVoiceCommandSnippetSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyVoiceCommandListSchema = z
  .array(LegacyVoiceCommandSchema)
  .max(LEGACY_VOICE_COMMAND_LIMIT)
  .refine((commands) => {
    if (commands.length > LEGACY_VOICE_COMMAND_LIMIT) return true;
    let total = 0;
    for (const command of commands) {
      if (
        command.trigger.length > LEGACY_VOICE_TRIGGER_MAX_LENGTH ||
        command.snippet.length > LEGACY_VOICE_SNIPPET_MAX_LENGTH
      ) {
        return true;
      }
      total += legacyUtf8ByteLength(command.trigger) + legacyUtf8ByteLength(command.snippet);
      if (total > LEGACY_VOICE_COMMANDS_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Voice commands exceed the total UTF-8 size limit');
export type LegacyVoiceCommand = z.infer<typeof LegacyLooseVoiceCommandSchema>;

const LEGACY_VOCABULARY_LIMIT = 1_000;
const LEGACY_VOCABULARY_VALUE_MAX_LENGTH = 200;
const LEGACY_VOCABULARY_VALUE_MAX_UTF8_BYTES = 400;
const LEGACY_VOCABULARY_TOTAL_MAX_UTF8_BYTES = 256_000;

export const LegacyLooseVocabularyEntrySchema = z
  .object({
    id: z.uuid(),
    value: z.string().trim().min(1).max(LEGACY_VOCABULARY_VALUE_MAX_LENGTH),
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyLooseVocabularyListSchema = z
  .array(LegacyLooseVocabularyEntrySchema)
  .max(LEGACY_VOCABULARY_LIMIT);
const LegacyVocabularyValueSchema = z
  .string()
  .trim()
  .min(1)
  .max(LEGACY_VOCABULARY_VALUE_MAX_LENGTH)
  .refine(
    (value) =>
      value.length > LEGACY_VOCABULARY_VALUE_MAX_LENGTH ||
      legacyUtf8ByteLength(value) <= LEGACY_VOCABULARY_VALUE_MAX_UTF8_BYTES,
    'Vocabulary entry is too large when encoded as UTF-8',
  )
  .refine((value) => /[\p{L}\p{N}]/u.test(value), 'Vocabulary must contain a letter or number')
  .refine(
    (value) => !hasLegacyControlCharacters(value, false),
    'Control characters are not allowed',
  );
export const LegacyVocabularyEntrySchema = z
  .object({
    id: z.uuid(),
    value: LegacyVocabularyValueSchema,
    createdAt: z.number().int().nonnegative(),
    updatedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyVocabularyListSchema = z
  .array(LegacyVocabularyEntrySchema)
  .max(LEGACY_VOCABULARY_LIMIT)
  .refine((entries) => {
    if (entries.length > LEGACY_VOCABULARY_LIMIT) return true;
    let total = 0;
    for (const entry of entries) {
      if (entry.value.length > LEGACY_VOCABULARY_VALUE_MAX_LENGTH) return true;
      total += legacyUtf8ByteLength(entry.value);
      if (total > LEGACY_VOCABULARY_TOTAL_MAX_UTF8_BYTES) return false;
    }
    return true;
  }, 'Custom vocabulary exceeds the total UTF-8 size limit');
export type LegacyVocabularyEntry = z.infer<typeof LegacyLooseVocabularyEntrySchema>;

const LegacyWelcomeStepSchema = z.union([
  z.literal(1),
  z.literal(2),
  z.literal(3),
  z.literal(4),
  z.literal(5),
  z.literal(6),
]);
const LegacyMicrophoneEvidenceSchema = z
  .object({
    boundDeviceId: LegacyMicrophoneIdSchema.nullable(),
    observedRms: z.number().positive().max(1),
    usableThreshold: z.number().positive().max(1),
    sampleCount: z.number().int().positive(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict()
  .refine((value) => value.observedRms >= value.usableThreshold, {
    message: 'Microphone evidence must contain a usable signal.',
  });
const LegacyActivationEvidenceSchema = z
  .object({
    activationKey: z.string().length(1),
    enabled: z.literal(true),
    helperProtocol: z.number().int().positive(),
    readinessGeneration: z.number().int().nonnegative(),
    observedAt: z.number().int().nonnegative(),
  })
  .strict();
const LegacyModelEvidenceSchema = z
  .object({
    modelId: LegacyWhisperModelIdSchema,
    manifestRevision: z.string().regex(/^[a-f0-9]{40}$/),
    verified: z.literal(true),
    verifiedAt: z.number().int().nonnegative(),
  })
  .strict();
export const LegacyWelcomeSettingsSchema = z
  .object({
    completedAt: z.number().int().nonnegative().nullable(),
    lastStep: LegacyWelcomeStepSchema,
    microphoneTested: z.boolean(),
    activationTested: z.boolean(),
    microphoneEvidence: LegacyMicrophoneEvidenceSchema.nullable().optional(),
    activationEvidence: LegacyActivationEvidenceSchema.nullable().optional(),
    modelEvidence: LegacyModelEvidenceSchema.nullable().optional(),
    revision: z.number().int().nonnegative().optional(),
  })
  .strict();
// V16 cleared these values before target validation, so its released migration accepted both the
// historical evidence shape and later shapes. Preserve that permissive source behavior.
export const LegacyWelcomeSettingsV16Schema = LegacyWelcomeSettingsSchema.extend({
  activationTested: z.unknown().optional(),
  activationEvidence: z.unknown().nullable().optional(),
  modelEvidence: z.unknown().nullable().optional(),
});

export interface LegacySettingsBase {
  readonly app:
    z.infer<typeof LegacyAppSettingsSchema> | z.infer<typeof LegacyExtendedAppSettingsSchema>;
  readonly recording?: z.infer<typeof LegacyRecordingSettingsSchema>;
  readonly transcription?: z.infer<typeof LegacyTranscriptionSettingsSchema>;
  readonly privacy?: z.infer<typeof LegacyPrivacyV7Schema>;
  readonly voiceCommands?: readonly LegacyVoiceCommand[];
  readonly customVocabulary?: readonly LegacyVocabularyEntry[];
  readonly smartProcessing?: {
    readonly selectedProviderId: LegacyProviderId;
    readonly providers: Partial<Record<LegacyProviderId, LegacyProviderDraft>>;
    readonly credentialEpochs?: Partial<Record<LegacyProviderId, number>>;
    readonly onScreenAwarenessEnabled?: boolean;
    readonly visionOverrides?: readonly {
      readonly providerId: 'generic-openai' | 'litellm';
      readonly binding: string;
      readonly modelId: string;
      readonly verifiedAt: number;
    }[];
  };
}

export interface LegacySettingsWithWelcomeProgress extends LegacySettingsBase {
  readonly welcome: {
    readonly completedAt: number | null;
    readonly lastStep: 1 | 2 | 3 | 4 | 5 | 6;
  };
}

function validateLegacyProviderDrafts(
  drafts: Partial<Record<LegacyProviderId, LegacyProviderDraft>>,
  context: z.core.$RefinementCtx,
): void {
  for (const [providerId, draft] of Object.entries(drafts)) {
    const parsedId = LegacyProviderIdSchema.safeParse(providerId);
    if (!parsedId.success) continue;
    const candidate =
      parsedId.data === 'bedrock' && draft.region === undefined
        ? { providerId: parsedId.data, ...draft, region: 'us-west-2' }
        : { providerId: parsedId.data, ...draft };
    const parsed = LegacyProviderConfigSchema.safeParse(candidate);
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

const LegacyProviderConfigSchema = z
  .object({
    providerId: LegacyProviderIdSchema,
    baseUrl: LegacyProviderBaseUrlSchema.optional(),
    modelId: LegacyProviderModelIdSchema.nullable().optional(),
    contextWindow: z.number().int().min(1).max(2_000_000).optional(),
    maxOutputTokens: z.number().int().min(1).max(16_384).optional(),
    timeoutMs: z.number().int().min(500).max(120_000).optional(),
    keepAlive: LegacyKeepAliveSchema.optional(),
    region: LegacyAwsRegionSchema.optional(),
    modelType: LegacyAzureModelTypeSchema.optional(),
    thinking: LegacyPiThinkingLevelSchema.optional(),
  })
  .strict()
  .superRefine((config, context) => {
    if (legacyConfigurableProviderIds.has(config.providerId) && config.baseUrl === undefined) {
      context.addIssue({
        code: 'custom',
        path: ['baseUrl'],
        message: 'A provider endpoint is required.',
      });
    }
    if (!legacyConfigurableProviderIds.has(config.providerId) && config.baseUrl !== undefined) {
      context.addIssue({
        code: 'custom',
        path: ['baseUrl'],
        message: 'Fixed providers do not accept endpoint overrides.',
      });
    }
    if (config.keepAlive !== undefined && config.providerId !== 'ollama') {
      context.addIssue({
        code: 'custom',
        path: ['keepAlive'],
        message: 'Keep alive is only supported by Ollama.',
      });
    }
    if (config.timeoutMs !== undefined && !legacyTimeoutProviderIds.has(config.providerId)) {
      context.addIssue({
        code: 'custom',
        path: ['timeoutMs'],
        message: 'A custom timeout is not supported by this provider.',
      });
    }
    if ((config.region !== undefined) !== (config.providerId === 'bedrock')) {
      context.addIssue({
        code: 'custom',
        path: ['region'],
        message: 'Region is required only for AWS Bedrock.',
      });
    }
    if (config.modelType !== undefined && config.providerId !== 'azure') {
      context.addIssue({
        code: 'custom',
        path: ['modelType'],
        message: 'Model type is supported only by Azure OpenAI.',
      });
    }
    if (config.thinking !== undefined && config.providerId !== 'pi') {
      context.addIssue({
        code: 'custom',
        path: ['thinking'],
        message: 'Thinking level is required only for Pi.',
      });
    }
  });

const LEGACY_COMPATIBILITY_LETTERS: Readonly<Record<string, string>> = Object.freeze({
  Ł: 'L',
  ł: 'l',
  Đ: 'D',
  đ: 'd',
  Ø: 'O',
  ø: 'o',
  Æ: 'AE',
  æ: 'ae',
  Œ: 'OE',
  œ: 'oe',
  Ð: 'D',
  ð: 'd',
  Þ: 'TH',
  þ: 'th',
});

function normalizeLegacyCommandText(value: string): string {
  return Array.from(value)
    .map((character) => LEGACY_COMPATIBILITY_LETTERS[character] ?? character)
    .join('')
    .normalize('NFKD')
    .replace(/(?<=\p{Script=Latin})\p{M}+/gu, '')
    .toLocaleLowerCase('en-US')
    .replace(/[\p{Pd}'’ʼ]+/gu, '')
    .replace(/\p{P}+/gu, ' ')
    .replace(/\s+/gu, ' ')
    .trim();
}

function hasLegacyControlCharacters(value: string, allowWhitespace: boolean): boolean {
  return Array.from(value).some((character) => {
    const code = character.codePointAt(0) ?? 0;
    if (allowWhitespace && (code === 9 || code === 10 || code === 13)) return false;
    return code < 32 || code === 127;
  });
}

function legacyUtf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}
