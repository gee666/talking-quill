import { z } from 'zod';
import {
  ProviderIdSchema,
  DestinationSchema,
  VisionCapabilitySchema,
  ProviderModelIdSchema,
} from './provider-config';

export {
  OPENAI_COMPATIBLE_PROVIDER_IDS,
  PI_THINKING_LEVELS,
  PiThinkingLevelSchema,
  type PiThinkingLevel,
  NATIVE_CLOUD_PROVIDER_IDS,
  PROVIDER_IDS,
  RUNNABLE_PROVIDER_IDS,
  ProviderIdSchema,
  RunnableProviderIdSchema,
  OpenAICompatibleProviderIdSchema,
  NativeCloudProviderIdSchema,
  type ProviderId,
  type RunnableProviderId,
  type OpenAICompatibleProviderId,
  type NativeCloudProviderId,
  DestinationSchema,
  type Destination,
  VisionCapabilitySchema,
  type VisionCapability,
  CONFIGURABLE_PROVIDER_IDS,
  AzureModelTypeSchema,
  AwsRegionSchema,
  PersistedProviderBaseUrlSchema,
  ProviderBaseUrlSchema,
  ProviderModelIdSchema,
  LegacyPiExtensionSourceSchema,
  LegacyPiExtensionSourcesSchema,
  PersistedPiExtensionSourceSchema,
  PersistedPiExtensionSourcesSchema,
  PiExtensionSourceSchema,
  PiExtensionSourcesSchema,
  PersistedProviderConfigSchema,
  ProviderConfigSchema,
  type ProviderConfig,
  type RunnableProviderConfig,
  RunnableProviderConfigSchema,
  parsePiNpmExtensionSource,
} from './provider-config';

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
      'piExtensionSources',
    ]),
    label: z.string().min(1).max(80),
    kind: z.enum(['url', 'secret', 'text', 'number', 'select', 'model', 'textarea']),
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
