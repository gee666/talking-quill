import { z } from 'zod';
import { AppStateSchema } from '../schemas/app-state';
import {
  ResetAcknowledgementTokenSchema,
  ResetApplicationDataRequestSchema,
  ResetApplicationDataResultSchema,
} from '../schemas/data-lifecycle';
import {
  ApplicationUpdateStateSchema,
  InfoLocationSchema,
  InfoPermissionSchema,
  InfoStatusSchema,
  ThirdPartyNoticesSchema,
  UpdateCheckResultSchema,
} from '../schemas/info';
import { WelcomeStateSchema, WelcomeStepSchema } from '../schemas/welcome';
import { ActivationTestStateSchema } from '../schemas/activation-test';
import { ProviderOperationIdSchema } from '../schemas/providers';
import { PublicSettingsPatchSchema, SettingsSchema } from '../schemas/settings';
import { SettingsTransferResultSchema } from '../schemas/settings-transfer';
import { ShortcutCaptureLeaseIdSchema } from '../schemas/shortcut-capture';
import {
  BuiltInDictationProfileIdSchema,
  CustomDictationProfileIdSchema,
  DictationProfileCreateSchema,
  DictationProfileIdSchema,
  DictationProfilePatchSchema,
  RESERVED_DICTATION_BINDING_ERROR,
  isReservedBindingForProfile,
} from '../schemas/dictation-profiles';
import { emptyRequest, acknowledgement, defineInvoke } from './invoke-definition';

export const applicationInvokes = Object.freeze({
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
  'info:update-state': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: ApplicationUpdateStateSchema,
  }),
  'info:apply-update': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: ApplicationUpdateStateSchema,
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
  'info:export-diagnostics': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: z.object({ status: z.enum(['cancelled', 'exported']) }).strict(),
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
    response: z.object({ leaseId: ShortcutCaptureLeaseIdSchema }).strict(),
  }),
  'shortcut-capture:stop': defineInvoke({
    roles: ['main'] as const,
    request: z.object({ leaseId: ShortcutCaptureLeaseIdSchema }).strict(),
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
          isReservedBindingForProfile(request.id, request.patch.shortcut)
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
  'profile:import-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: SettingsTransferResultSchema,
  }),
  'profile:export-file': defineInvoke({
    roles: ['main'] as const,
    request: emptyRequest,
    response: SettingsTransferResultSchema,
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
});
