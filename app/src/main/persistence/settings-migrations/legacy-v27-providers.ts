import { z } from 'zod';

// Frozen released contract. Do not import mutable current schemas.
import { noControlCharacters } from './legacy-v27-text';

export const LegacyProviderIdV27Schema = z.enum([
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
const PersistedProviderBaseUrl = z
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
const PersistedPiExtensionSource = z
  .string()
  .trim()
  .min(1)
  .max(512)
  .regex(/^(?!-).+$/u)
  .refine(noControlCharacters, 'Extension paths cannot contain control characters.')
  .refine(
    (value) => !/^(?:[\\/]{2}|[\\/]\?\?[\\/])/u.test(value),
    'UNC, network, and namespaced paths cannot be used for Pi extensions.',
  );
export const LegacyProviderDraftV27Schema = z
  .object({
    baseUrl: PersistedProviderBaseUrl.optional(),
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
    region: z
      .string()
      .regex(
        /^(?:af|ap|ca|eu|il|me|mx|sa|us)-(?:central|east|north|northeast|northwest|south|southeast|southwest|west)-\d$/,
      )
      .max(32)
      .optional(),
    modelType: z.enum(['default', 'reasoning']).optional(),
    thinking: z.enum(['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max']).optional(),
    piExtensionSources: z.array(PersistedPiExtensionSource).max(8).optional(),
  })
  .strict();
const configurableProviderIds = new Set([
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
const timeoutProviderIds = new Set(['openrouter', 'novita', 'cometapi']);
const ProviderDrafts = z
  .partialRecord(LegacyProviderIdV27Schema, LegacyProviderDraftV27Schema)
  .superRefine((drafts, context) => {
    for (const [providerId, draft] of Object.entries(drafts)) {
      const issues: [string, string][] = [];
      if (configurableProviderIds.has(providerId) && draft.baseUrl === undefined)
        issues.push(['baseUrl', 'A provider endpoint is required.']);
      if (!configurableProviderIds.has(providerId) && draft.baseUrl !== undefined)
        issues.push(['baseUrl', 'Fixed providers do not accept endpoint overrides.']);
      if (draft.keepAlive !== undefined && providerId !== 'ollama')
        issues.push(['keepAlive', 'Keep alive is only supported by Ollama.']);
      if (draft.timeoutMs !== undefined && !timeoutProviderIds.has(providerId))
        issues.push(['timeoutMs', 'A custom timeout is not supported by this provider.']);
      if ((draft.region !== undefined) !== (providerId === 'bedrock'))
        issues.push(['region', 'Region is required only for AWS Bedrock.']);
      if (draft.modelType !== undefined && providerId !== 'azure')
        issues.push(['modelType', 'Model type is supported only by Azure OpenAI.']);
      if (draft.thinking !== undefined && providerId !== 'pi')
        issues.push(['thinking', 'Thinking level is required only for Pi.']);
      if (draft.piExtensionSources !== undefined && providerId !== 'pi')
        issues.push(['piExtensionSources', 'Extension sources are supported only by Pi.']);
      for (const [field, message] of issues)
        context.addIssue({ code: 'custom', path: [providerId, field], message });
    }
  });
export const LegacySmartProcessingV27Schema = z
  .object({
    selectedProviderId: LegacyProviderIdV27Schema,
    providers: ProviderDrafts,
    credentialEpochs: z.partialRecord(
      LegacyProviderIdV27Schema,
      z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
    ),
    piInstallationPath: z.string().trim().min(1).max(8192).nullable(),
    onScreenAwarenessEnabled: z.boolean(),
    visionOverrides: z
      .array(
        z
          .object({
            providerId: z.enum(['generic-openai', 'litellm']),
            binding: z.string().min(1).max(2048),
            modelId: z.string().trim().min(1).max(512),
            verifiedAt: z.number().int().nonnegative(),
          })
          .strict(),
      )
      .max(64),
  })
  .strict();
