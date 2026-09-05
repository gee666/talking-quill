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
export const LegacyPiExtensionSourceSchema = z
  .string()
  .trim()
  .min(1)
  .max(512)
  .regex(/^(?!-).+$/u)
  .refine(noControlCharacters, 'Extension paths cannot contain control characters.');
export const LegacyPiExtensionSourcesSchema = z.array(LegacyPiExtensionSourceSchema).max(8);
export const PersistedPiExtensionSourceSchema = LegacyPiExtensionSourceSchema.refine(
  isNonNetworkPiExtensionPath,
  'UNC, network, and namespaced paths cannot be used for Pi extensions.',
);
export const PersistedPiExtensionSourcesSchema = z.array(PersistedPiExtensionSourceSchema).max(8);
export const PiExtensionSourceSchema = PersistedPiExtensionSourceSchema.refine(
  isLocalPiExtensionPathOrNpmPackage,
  'Enter a local extension path or an exact installed npm:package name. URLs, git sources, and version ranges are not supported.',
).refine(
  (value) => parsePiNpmExtensionSource(value) !== null || noWindowsCommandCharacters(value),
  'Extension paths cannot contain Windows command characters: " % ! & | < > ^ ( ).',
);
export const PiExtensionSourcesSchema = z.array(PiExtensionSourceSchema).max(8);

export const PersistedProviderConfigSchema = createProviderConfigSchema(
  PersistedProviderBaseUrlSchema,
  PersistedPiExtensionSourcesSchema,
);
export const ProviderConfigSchema = createProviderConfigSchema(
  ProviderBaseUrlSchema,
  PiExtensionSourcesSchema,
);
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

function createProviderConfigSchema(
  baseUrlSchema: z.ZodType<string>,
  piExtensionSourcesSchema: z.ZodType<string[]>,
) {
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
      piExtensionSources: piExtensionSourcesSchema.optional(),
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
      if (config.piExtensionSources !== undefined && config.providerId !== 'pi') {
        context.addIssue({
          code: 'custom',
          path: ['piExtensionSources'],
          message: 'Extension sources are supported only by Pi.',
        });
      }
    });
}

function noControlCharacters(value: string): boolean {
  for (const character of value) {
    const codePoint = character.codePointAt(0) ?? 0;
    if (codePoint < 0x20 || codePoint === 0x7f) return false;
  }
  return true;
}

function isNonNetworkPiExtensionPath(value: string): boolean {
  return !/^(?:[\\/]{2}|[\\/]\?\?[\\/])/u.test(value);
}

export function parsePiNpmExtensionSource(value: string): string | null {
  const match = /^npm:((?:@[A-Za-z0-9~-][A-Za-z0-9._~-]*\/)?[A-Za-z0-9~-][A-Za-z0-9._~-]*)$/u.exec(
    value,
  );
  return match?.[1] ?? null;
}

function isLocalPiExtensionPathOrNpmPackage(value: string): boolean {
  return parsePiNpmExtensionSource(value) !== null || isLocalPiExtensionPath(value);
}

function isLocalPiExtensionPath(value: string): boolean {
  if (/^[A-Za-z]:[\\/]/u.test(value)) return true;
  return (
    !/^[A-Za-z][A-Za-z0-9+.-]*:/u.test(value) &&
    !value.startsWith('@') &&
    !/^[^\\/]+@[^\\/]+:/u.test(value)
  );
}

function noWindowsCommandCharacters(value: string): boolean {
  return !/["%!&|<>^()]/u.test(value);
}
