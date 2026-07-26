import { z } from 'zod';

export const OPENAI_COMPATIBLE_PROVIDER_IDS = [
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
] as const;

export const PI_THINKING_LEVELS = [
  'off',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
] as const;
export const PiThinkingLevelSchema = z.enum(PI_THINKING_LEVELS);
export type PiThinkingLevel = z.infer<typeof PiThinkingLevelSchema>;

export const NATIVE_CLOUD_PROVIDER_IDS = [
  'anthropic',
  'gemini',
  'azure',
  'bedrock',
  'cohere',
] as const;
export const PROVIDER_IDS = [
  ...OPENAI_COMPATIBLE_PROVIDER_IDS,
  'ollama',
  'pi',
  ...NATIVE_CLOUD_PROVIDER_IDS,
] as const;

export const RUNNABLE_PROVIDER_IDS = PROVIDER_IDS;

export const ProviderIdSchema = z.enum(PROVIDER_IDS);
export const RunnableProviderIdSchema = z.enum(RUNNABLE_PROVIDER_IDS);
export const OpenAICompatibleProviderIdSchema = z.enum(OPENAI_COMPATIBLE_PROVIDER_IDS);
export const NativeCloudProviderIdSchema = z.enum(NATIVE_CLOUD_PROVIDER_IDS);
export type ProviderId = z.infer<typeof ProviderIdSchema>;
export type RunnableProviderId = z.infer<typeof RunnableProviderIdSchema>;
export type OpenAICompatibleProviderId = z.infer<typeof OpenAICompatibleProviderIdSchema>;
export type NativeCloudProviderId = z.infer<typeof NativeCloudProviderIdSchema>;

export const DestinationSchema = z.enum(['local', 'lan', 'cloud']);
export type Destination = z.infer<typeof DestinationSchema>;

export const VisionCapabilitySchema = z.enum(['supported', 'unsupported', 'unknown']);
export type VisionCapability = z.infer<typeof VisionCapabilitySchema>;

export const CONFIGURABLE_PROVIDER_IDS = [
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
] as const satisfies readonly ProviderId[];

export const AzureModelTypeSchema = z.enum(['default', 'reasoning']);
export const AwsRegionSchema = z
  .string()
  .regex(
    /^(?:af|ap|ca|eu|il|me|mx|sa|us)-(?:central|east|north|northeast|northwest|south|southeast|southwest|west)-\d$/,
  )
  .max(32);

const configurableProviderIds = new Set<ProviderId>(CONFIGURABLE_PROVIDER_IDS);
const timeoutProviderIds = new Set<ProviderId>(['openrouter', 'novita', 'cometapi']);

const CredentialFreeProviderUrlSchema = z
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

// Version 19 settings accepted any URL protocol. Keep that persisted contract readable while all
// new provider configuration must use a transport-supported HTTP endpoint.
export const PersistedProviderBaseUrlSchema = CredentialFreeProviderUrlSchema;
export const ProviderBaseUrlSchema = CredentialFreeProviderUrlSchema.refine((value) => {
  if (!URL.canParse(value)) return false;
  const endpoint = new URL(value);
  return (
    (endpoint.protocol === 'http:' || endpoint.protocol === 'https:') &&
    endpoint.hostname.length > 0
  );
}, 'Provider endpoints must use HTTP or HTTPS and include a hostname.');
export const ProviderModelIdSchema = z.string().trim().min(1).max(512);

export const PersistedProviderConfigSchema = createProviderConfigSchema(
  PersistedProviderBaseUrlSchema,
);
export const ProviderConfigSchema = createProviderConfigSchema(ProviderBaseUrlSchema);
export type ProviderConfig = z.infer<typeof ProviderConfigSchema>;
export type RunnableProviderConfig = Omit<ProviderConfig, 'providerId'> & {
  readonly providerId: RunnableProviderId;
};
export const RunnableProviderConfigSchema = ProviderConfigSchema.pipe(
  z.custom<RunnableProviderConfig>(
    (config) =>
      typeof config === 'object' &&
      config !== null &&
      'providerId' in config &&
      RunnableProviderIdSchema.safeParse(config.providerId).success,
    'The provider is not runnable.',
  ),
);

function createProviderConfigSchema(baseUrlSchema: z.ZodType<string>) {
  return z
    .object({
      providerId: ProviderIdSchema,
      baseUrl: baseUrlSchema.optional(),
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
    .strict()
    .superRefine((config, context) => {
      if (configurableProviderIds.has(config.providerId) && config.baseUrl === undefined) {
        context.addIssue({
          code: 'custom',
          path: ['baseUrl'],
          message: 'A provider endpoint is required.',
        });
      }
      if (!configurableProviderIds.has(config.providerId) && config.baseUrl !== undefined) {
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
      if (config.timeoutMs !== undefined && !timeoutProviderIds.has(config.providerId)) {
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
}

export const ModelInfoSchema = z
  .object({
    id: ProviderModelIdSchema,
    name: z.string().trim().min(1).max(512),
    contextWindow: z.number().int().positive().max(2_000_000).nullable(),
    vision: VisionCapabilitySchema,
  })
  .strict();
export type ModelInfo = z.infer<typeof ModelInfoSchema>;

export const MAX_PROVIDER_INPUT_UTF8_BYTES = 480 * 1_024;
// Leaves deterministic headroom for prompt JSON inside the transport's 512 KiB wire cap.
export const MAX_PROVIDER_IMAGE_BYTES = 256 * 1_024;

export const ProviderImageSchema = z
  .object({
    mimeType: z.literal('image/jpeg'),
    base64: z
      .string()
      .min(4)
      .max(Math.ceil((MAX_PROVIDER_IMAGE_BYTES * 4) / 3) + 4)
      .regex(/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/)
      .refine((value) => decodedBase64Length(value) <= MAX_PROVIDER_IMAGE_BYTES),
  })
  .strict();
export type ProviderImage = z.infer<typeof ProviderImageSchema>;

export const ProviderCompletionRequestSchema = z
  .object({
    input: z
      .string()
      .min(1)
      .max(MAX_PROVIDER_INPUT_UTF8_BYTES)
      .refine(
        (value) => new TextEncoder().encode(value).byteLength <= MAX_PROVIDER_INPUT_UTF8_BYTES,
        'Provider input exceeds the UTF-8 byte limit.',
      ),
    modelId: ProviderModelIdSchema.optional(),
    temperature: z.number().min(0).max(2).default(0.2),
    maxOutputTokens: z.number().int().min(1).max(16_384).optional(),
    image: ProviderImageSchema.optional(),
  })
  .strict();
export type ProviderCompletionRequest = z.input<typeof ProviderCompletionRequestSchema>;

export const ProviderValidationResultSchema = z
  .object({
    ok: z.literal(true),
    destination: DestinationSchema,
    modelCount: z.number().int().nonnegative(),
  })
  .strict();
export type ProviderValidationResult = z.infer<typeof ProviderValidationResultSchema>;

export const ProviderFieldSchema = z
  .object({
    key: z.enum([
      'baseUrl',
      'credential',
      'modelId',
      'contextWindow',
      'maxOutputTokens',
      'timeoutMs',
      'keepAlive',
      'region',
      'modelType',
      'thinking',
    ]),
    label: z.string().min(1).max(80),
    kind: z.enum(['url', 'secret', 'text', 'number', 'select', 'model']),
    required: z.boolean(),
    secret: z.boolean(),
    placeholder: z.string().max(256).optional(),
    description: z.string().min(1).max(320).optional(),
    defaultValue: z.union([z.string().max(256), z.number()]).optional(),
    min: z.number().optional(),
    max: z.number().optional(),
    options: z
      .array(
        z
          .object({
            value: z.union([z.string().max(64), z.number()]),
            label: z.string().min(1).max(80),
          })
          .strict(),
      )
      .max(32)
      .optional(),
  })
  .strict();
export type ProviderField = z.infer<typeof ProviderFieldSchema>;

export const ProviderCatalogEntrySchema = z
  .object({
    id: ProviderIdSchema,
    displayName: z.string().min(1).max(80),
    description: z.string().min(1).max(240),
    logo: z.string().regex(/^[a-z0-9-]+\.(?:png|jpeg)$/),
    destinationHint: DestinationSchema,
    defaultModel: ProviderModelIdSchema.nullable(),
    modelDiscovery: z.enum(['remote', 'provider-managed', 'azure-deployment']),
    fields: z.array(ProviderFieldSchema).max(8),
  })
  .strict();
export type ProviderCatalogEntry = z.infer<typeof ProviderCatalogEntrySchema>;

export const PublicProviderErrorCodeSchema = z.enum([
  'INVALID_CONFIG',
  'STALE_CONFIG',
  'MISSING_CREDENTIAL',
  'SECURITY_BLOCKED',
  'UNAVAILABLE',
  'PI_NOT_FOUND',
  'PI_CONFIG_INVALID',
  'PI_INCOMPATIBLE',
  'PI_LAUNCH_FAILED',
  'AUTHENTICATION_FAILED',
  'RATE_LIMITED',
  'MODEL_NOT_FOUND',
  'NO_MODELS',
  'TIMEOUT',
  'CANCELLED',
  'REQUEST_TOO_LARGE',
  'RESPONSE_TOO_LARGE',
  'INVALID_RESPONSE',
  'REMOTE_FAILURE',
]);
export type PublicProviderErrorCode = z.infer<typeof PublicProviderErrorCodeSchema>;

export const PublicProviderErrorSchema = z
  .object({
    code: PublicProviderErrorCodeSchema,
    message: z.string().min(1).max(160),
    retryable: z.boolean(),
  })
  .strict();
export type PublicProviderError = z.infer<typeof PublicProviderErrorSchema>;

function decodedBase64Length(value: string): number {
  const padding = value.endsWith('==') ? 2 : value.endsWith('=') ? 1 : 0;
  return (value.length / 4) * 3 - padding;
}

export const VisionVerificationSchema = z.object({ verificationId: z.uuid() }).strict();
export type VisionVerification = z.infer<typeof VisionVerificationSchema>;

export const ProviderOperationIdSchema = z
  .string()
  .min(8)
  .max(64)
  .regex(/^[a-zA-Z0-9_-]+$/);
