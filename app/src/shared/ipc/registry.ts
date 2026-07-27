import { z } from 'zod';
import { AppStateSchema } from '../schemas/app-state';
import {
  ResetAcknowledgementTokenSchema,
  ResetApplicationDataRequestSchema,
  ResetApplicationDataResultSchema,
} from '../schemas/data-lifecycle';
import {
  InfoLocationSchema,
  InfoPermissionSchema,
  InfoStatusSchema,
  ThirdPartyNoticesSchema,
  UpdateCheckResultSchema,
} from '../schemas/info';
import { WelcomeStateSchema, WelcomeStepSchema } from '../schemas/welcome';
import { ActivationTestStateSchema } from '../schemas/activation-test';
import {
  MicrophoneDeviceListSchema,
  MicrophoneLevelSchema,
  MicrophoneTestStateSchema,
} from '../schemas/audio';
import {
  CredentialSecretSchema,
  ProviderCredentialBindingTokenSchema,
  ProviderCredentialStateSchema,
} from '../schemas/credentials';
import { EchoSessionSnapshotSchema } from '../schemas/echo-session';
import {
  VoiceCommandIdSchema,
  VoiceCommandInputSchema,
  VoiceCommandListSchema,
  VoiceCommandMatchSchema,
  VoiceCommandSchema,
  VoiceCommandUpdateSchema,
} from '../schemas/commands';
import {
  HistoryChangedSchema,
  HistoryCopyResultSchema,
  HistoryDeleteAllResultSchema,
  HistoryDeleteResultSchema,
  HistoryIdSchema,
  HistoryListRequestSchema,
  HistoryPageSchema,
  HistoryThumbnailSchema,
} from '../schemas/history';
import {
  VocabularyEntrySchema,
  VocabularyFileResultSchema,
  VocabularyIdSchema,
  VocabularyListSchema,
  VocabularyValueSchema,
} from '../schemas/vocabulary';
import { WhisperModelIdSchema } from '../schemas/model-manifest';
import {
  DestinationSchema,
  ModelInfoSchema,
  PROVIDER_IDS,
  ProviderCatalogEntrySchema,
  ProviderModelIdSchema,
  ProviderOperationIdSchema,
  RunnableProviderConfigSchema,
  ProviderValidationResultSchema,
  PublicProviderErrorCodeSchema,
  RunnableProviderIdSchema,
  VisionCapabilitySchema,
  VisionVerificationSchema,
} from '../schemas/providers';
import { PublicSettingsPatchSchema, SettingsSchema } from '../schemas/settings';
import {
  BuiltInDictationProfileIdSchema,
  CustomDictationProfileIdSchema,
  DictationProfileCreateSchema,
  DictationProfileIdSchema,
  DictationProfilePatchSchema,
  RESERVED_DICTATION_BINDING_ERROR,
  isReservedBindingForAnotherProfile,
} from '../schemas/dictation-profiles';
import {
  PiInstallationBrowseResultSchema,
  PiInstallationSaveRequestSchema,
  PiInstallationStatusSchema,
} from '../schemas/pi-installation';
import {
  ModelDeleteResultSchema,
  ModelProgressSchema,
  ModelStatusSchema,
} from '../schemas/transcription';
import type { WindowRole } from '../constants/app';
import { CapturePortDescriptorSchema } from './capture-port';

const emptyRequest = z.object({}).strict();
const acknowledgement = z.object({ accepted: z.literal(true) }).strict();
const ProviderCatalogSchema = z
  .array(ProviderCatalogEntrySchema)
  .length(PROVIDER_IDS.length)
  .refine(
    (providers) => providers.every((provider, index) => provider.id === PROVIDER_IDS[index]),
    'Provider catalog must contain every provider in canonical order',
  );

const defineInvoke = <
  const Roles extends readonly WindowRole[],
  Request extends z.ZodType,
  Response extends z.ZodType,
>(definition: {
  readonly roles: Roles;
  readonly request: Request;
  readonly response: Response;
}) => Object.freeze(definition);

export const invokeRegistry = Object.freeze({
  'bootstrap:get': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z
      .object({
        appVersion: z.string().min(1),
        sourceRevision: z.string().regex(/^[0-9a-f]{7,12}$/u),
        platform: z.string().min(1),
        state: AppStateSchema,
        settings: SettingsSchema,
      })
      .strict(),
  }),
  'welcome:set-step': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ step: WelcomeStepSchema }).strict(),
    response: WelcomeStateSchema,
  }),
  'welcome:complete': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: WelcomeStateSchema,
  }),
  'info:status': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: InfoStatusSchema,
  }),
  'info:check-update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ operationId: ProviderOperationIdSchema }).strict(),
    response: UpdateCheckResultSchema,
  }),
  'info:cancel-update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ operationId: ProviderOperationIdSchema }).strict(),
    response: z.object({ cancelled: z.boolean() }).strict(),
  }),
  'info:open-permission': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ permission: InfoPermissionSchema }).strict(),
    response: acknowledgement,
  }),
  'info:open-location': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ location: InfoLocationSchema }).strict(),
    response: acknowledgement,
  }),
  'info:open-release': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ url: z.url().max(2_048) }).strict(),
    response: acknowledgement,
  }),
  'info:notices': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.object({ text: ThirdPartyNoticesSchema }).strict(),
  }),
  'activation-test:start': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: ActivationTestStateSchema,
  }),
  'activation-test:stop': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: ActivationTestStateSchema,
  }),
  'shortcut-capture:start': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'shortcut-capture:stop': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'app:set-enabled': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ enabled: z.boolean() }).strict(),
    response: AppStateSchema,
  }),
  'settings:update': defineInvoke({
    roles: ['main'] as const,
    request: PublicSettingsPatchSchema,
    response: SettingsSchema,
  }),
  'profile:create': defineInvoke({
    roles: ['main'] as const,
    request: DictationProfileCreateSchema,
    response: SettingsSchema,
  }),
  'profile:update': defineInvoke({
    roles: ['main'] as const,
    request: z
      .object({ id: DictationProfileIdSchema, patch: DictationProfilePatchSchema })
      .strict()
      .superRefine((request, context) => {
        if (
          request.patch.shortcut !== undefined &&
          isReservedBindingForAnotherProfile(request.id, request.patch.shortcut)
        ) {
          context.addIssue({
            code: 'custom',
            path: ['patch', 'shortcut'],
            message: RESERVED_DICTATION_BINDING_ERROR,
          });
        }
      }),
    response: SettingsSchema,
  }),
  'profile:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: CustomDictationProfileIdSchema }).strict(),
    response: SettingsSchema,
  }),
  'profile:reset': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: BuiltInDictationProfileIdSchema }).strict(),
    response: SettingsSchema,
  }),
  'data:reset-all': defineInvoke({
    roles: ['main'] as const,
    request: ResetApplicationDataRequestSchema,
    response: ResetApplicationDataResultSchema,
  }),
  'data:reset-renderer-ack': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ acknowledgementToken: ResetAcknowledgementTokenSchema }).strict(),
    response: acknowledgement,
  }),
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
  'history:list': defineInvoke({
    roles: ['main'] as const,
    request: HistoryListRequestSchema,
    response: HistoryPageSchema,
  }),
  'history:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryDeleteResultSchema,
  }),
  'history:delete-all': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: HistoryDeleteAllResultSchema,
  }),
  'history:copy': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryCopyResultSchema,
  }),
  'history:thumbnail': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: HistoryIdSchema }).strict(),
    response: HistoryThumbnailSchema,
  }),
  'model:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.array(ModelStatusSchema),
  }),
  'model:status': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema, verify: z.boolean().optional() }).strict(),
    response: ModelStatusSchema,
  }),
  'model:download': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:pause': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:cancel': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:retry': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelStatusSchema,
  }),
  'model:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ modelId: WhisperModelIdSchema }).strict(),
    response: ModelDeleteResultSchema,
  }),
  'window:minimize': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'window:toggle-maximize': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.object({ maximized: z.boolean() }).strict(),
  }),
  'window:close': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:ready': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: EchoSessionSnapshotSchema,
  }),
  'widget:stop': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:cancel': defineInvoke({
    roles: ['widget'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'widget:set-interactive': defineInvoke({
    roles: ['widget'] as const,
    request: z.object({ interactive: z.boolean() }).strict(),
    response: acknowledgement,
  }),
  'capture:ready': defineInvoke({
    roles: ['capture'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'recording:get-devices': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneDeviceListSchema,
  }),
  'recording:start-test': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneTestStateSchema,
  }),
  'recording:stop-test': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: MicrophoneTestStateSchema,
  }),
  'recording:open-microphone-settings': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: acknowledgement,
  }),
  'commands:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VoiceCommandListSchema,
  }),
  'commands:create': defineInvoke({
    roles: ['main'] as const,
    request: VoiceCommandInputSchema,
    response: VoiceCommandSchema,
  }),
  'commands:update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VoiceCommandIdSchema, patch: VoiceCommandUpdateSchema }).strict(),
    response: VoiceCommandSchema,
  }),
  'commands:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VoiceCommandIdSchema }).strict(),
    response: z.object({ deleted: z.boolean() }).strict(),
  }),
  'commands:preview': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ transcript: z.string().max(10_000) }).strict(),
    response: VoiceCommandMatchSchema.nullable(),
  }),
  'vocabulary:list': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyListSchema,
  }),
  'vocabulary:create': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ value: VocabularyValueSchema }).strict(),
    response: VocabularyEntrySchema,
  }),
  'vocabulary:update': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VocabularyIdSchema, value: VocabularyValueSchema }).strict(),
    response: VocabularyEntrySchema,
  }),
  'vocabulary:delete': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ id: VocabularyIdSchema }).strict(),
    response: z.object({ deleted: z.boolean() }).strict(),
  }),
  'vocabulary:import-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyFileResultSchema,
  }),
  'vocabulary:export-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: VocabularyFileResultSchema,
  }),
});

const defineEvent = <
  const Roles extends readonly WindowRole[],
  Payload extends z.ZodType,
>(definition: {
  readonly roles: Roles;
  readonly payload: Payload;
}) => Object.freeze(definition);

export const eventRegistry = Object.freeze({
  'data:reset-accepted': defineEvent({
    roles: ['main'] as const,
    payload: ResetApplicationDataResultSchema,
  }),
  'activation-test:changed': defineEvent({
    roles: ['main'] as const,
    payload: ActivationTestStateSchema,
  }),
  'app:state-changed': defineEvent({
    roles: ['main'] as const,
    payload: AppStateSchema,
  }),
  'settings:changed': defineEvent({
    roles: ['main'] as const,
    payload: SettingsSchema,
  }),
  'history:changed': defineEvent({
    roles: ['main'] as const,
    payload: HistoryChangedSchema,
  }),
  'model:progress': defineEvent({
    roles: ['main'] as const,
    payload: ModelProgressSchema,
  }),
  'window:maximized-changed': defineEvent({
    roles: ['main'] as const,
    payload: z.object({ maximized: z.boolean() }).strict(),
  }),
  'recording:devices-changed': defineEvent({
    roles: ['main'] as const,
    payload: MicrophoneDeviceListSchema,
  }),
  'recording:test-level': defineEvent({
    roles: ['main'] as const,
    payload: MicrophoneLevelSchema,
  }),
  'recording:test-state-changed': defineEvent({
    roles: ['main'] as const,
    payload: MicrophoneTestStateSchema,
  }),
  'echo:session-changed': defineEvent({
    roles: ['main', 'widget'] as const,
    payload: EchoSessionSnapshotSchema,
  }),
});

export const portTransferRegistry = Object.freeze({
  'capture:port': Object.freeze({
    roles: ['capture'] as const,
    descriptor: CapturePortDescriptorSchema,
  }),
});

export const PublicErrorSchema = z
  .object({
    code: z.union([
      z.enum(['BAD_REQUEST', 'FORBIDDEN', 'UNAVAILABLE', 'NOT_FOUND', 'INTERNAL']),
      PublicProviderErrorCodeSchema,
    ]),
    message: z.string().min(1).max(240),
  })
  .strict();

export const failureResponseSchema = z
  .object({ ok: z.literal(false), error: PublicErrorSchema })
  .strict();

export type InvokeChannel = keyof typeof invokeRegistry;
export type EventChannel = keyof typeof eventRegistry;
export type PortTransferChannel = keyof typeof portTransferRegistry;
export type InvokeRequest<Channel extends InvokeChannel> = z.infer<
  (typeof invokeRegistry)[Channel]['request']
>;
export type InvokeResponse<Channel extends InvokeChannel> = z.infer<
  (typeof invokeRegistry)[Channel]['response']
>;
export type EventPayload<Channel extends EventChannel> = z.infer<
  (typeof eventRegistry)[Channel]['payload']
>;
export type PortTransferDescriptor<Channel extends PortTransferChannel> = z.infer<
  (typeof portTransferRegistry)[Channel]['descriptor']
>;
export type PublicError = z.infer<typeof PublicErrorSchema>;
export type WireResponse<Channel extends InvokeChannel> =
  | { readonly ok: true; readonly data: InvokeResponse<Channel> }
  | { readonly ok: false; readonly error: PublicError };

export function successResponseSchema<Channel extends InvokeChannel>(channel: Channel) {
  return z
    .object({ ok: z.literal(true), data: invokeRegistry[channel].response })
    .strict() as unknown as z.ZodType<{
    readonly ok: true;
    readonly data: InvokeResponse<Channel>;
  }>;
}
