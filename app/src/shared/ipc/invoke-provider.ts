import { z } from 'zod';
import {
  CredentialSecretSchema,
  ProviderCredentialBindingTokenSchema,
  ProviderCredentialStateSchema,
} from '../schemas/credentials';
import {
  DestinationSchema,
  ModelInfoSchema,
  PROVIDER_IDS,
  ProviderCatalogEntrySchema,
  ProviderModelIdSchema,
  ProviderOperationIdSchema,
  RunnableProviderConfigSchema,
  ProviderValidationResultSchema,
  RunnableProviderIdSchema,
  VisionCapabilitySchema,
  VisionVerificationSchema,
} from '../schemas/providers';
import { SettingsSchema } from '../schemas/settings';
import {
  PiInstallationBrowseResultSchema,
  PiInstallationSaveRequestSchema,
  PiInstallationStatusSchema,
} from '../schemas/pi-installation';
import { emptyRequest, defineInvoke } from './invoke-definition';

const ProviderCatalogSchema = z
  .array(ProviderCatalogEntrySchema)
  .length(PROVIDER_IDS.length)
  .refine(
    (providers) => providers.every((provider, index) => provider.id === PROVIDER_IDS[index]),
    'Provider catalog must contain every provider in canonical order',
  );

export const providerInvokes = Object.freeze({
  'provider:catalog': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.object({ providers: ProviderCatalogSchema }).strict(),
  }),
  'provider:pi-installation-status': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: PiInstallationStatusSchema,
  }),
  'provider:pi-installation-save': defineInvoke({
    roles: ['main'] as const,
    request: PiInstallationSaveRequestSchema,
    response: PiInstallationStatusSchema,
  }),
  'provider:pi-installation-browse': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: PiInstallationBrowseResultSchema,
  }),
  'provider:config-save': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ config: RunnableProviderConfigSchema }).strict(),
    response: z
      .object({
        settings: SettingsSchema,
        credentialState: ProviderCredentialStateSchema,
      })
      .strict(),
  }),
  'provider:secret-set': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({
        providerId: RunnableProviderIdSchema,
        expectedBindingToken: ProviderCredentialBindingTokenSchema,
        secret: CredentialSecretSchema,
      })
      .strict(),
    response: ProviderCredentialStateSchema,
  }),
  'provider:secret-status': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ providerId: RunnableProviderIdSchema }).strict(),
    response: ProviderCredentialStateSchema,
  }),
  'provider:secret-delete': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({
        providerId: RunnableProviderIdSchema,
        expectedBindingToken: ProviderCredentialBindingTokenSchema,
      })
      .strict(),
    response: ProviderCredentialStateSchema,
  }),
  'provider:list-models': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({
        providerId: RunnableProviderIdSchema,
        operationId: ProviderOperationIdSchema,
        refresh: z.boolean(),
      })
      .strict(),
    response: z
      .object({
        providerId: RunnableProviderIdSchema,
        models: z.array(ModelInfoSchema).max(10_000),
      })
      .strict(),
  }),
  'provider:test-connection': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({ providerId: RunnableProviderIdSchema, operationId: ProviderOperationIdSchema })
      .strict(),
    response: ProviderValidationResultSchema,
  }),
  'provider:destination': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({ providerId: RunnableProviderIdSchema, operationId: ProviderOperationIdSchema })
      .strict(),
    response: z.object({ destination: DestinationSchema }).strict(),
  }),
  'provider:cancel': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ operationId: ProviderOperationIdSchema }).strict(),
    response: z.object({ cancelled: z.boolean() }).strict(),
  }),
  'provider:osa-status': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z
      .object({
        providerId: RunnableProviderIdSchema,
        modelId: ProviderModelIdSchema.nullable(),
        capability: VisionCapabilitySchema,
        manualTestAllowed: z.boolean(),
        screenPermission: z.enum(['granted', 'denied', 'unknown']),
      })
      .strict(),
  }),
  'provider:osa-set': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ enabled: z.boolean() }).strict(),
    response: SettingsSchema,
  }),
  'provider:vision-test': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({
        operationId: ProviderOperationIdSchema,
        nonce: z.string().regex(/^[A-Z0-9-]{8,48}$/),
      })
      .strict(),
    response: VisionVerificationSchema,
  }),
  'provider:vision-confirm': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({
        operationId: ProviderOperationIdSchema,
        verificationId: z.uuid(),
      })
      .strict(),
    response: SettingsSchema,
  }),
});
