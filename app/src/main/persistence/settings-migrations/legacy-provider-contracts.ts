import { z } from 'zod';

// Frozen released contract. Do not import mutable current schemas.
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
