import { z } from 'zod';
import { AppStateSchema } from '../schemas/app-state';
import { ResetApplicationDataResultSchema } from '../schemas/data-lifecycle';
import { ApplicationUpdateStateSchema } from '../schemas/info';
import { ActivationTestStateSchema } from '../schemas/activation-test';
import {
  MicrophoneDeviceListSchema,
  MicrophoneLevelSchema,
  MicrophoneTestStateSchema,
} from '../schemas/audio';
import { EchoSessionSnapshotSchema } from '../schemas/echo-session';
import { HistoryChangedSchema } from '../schemas/history';
import { PublicProviderErrorCodeSchema } from '../schemas/providers';
import { SettingsSchema } from '../schemas/settings';
import { ModelProgressSchema } from '../schemas/transcription';
import type { WindowRole } from '../constants/app';
import { CapturePortDescriptorSchema } from './capture-port';
import { applicationInvokes } from './invoke-application';
import { providerInvokes } from './invoke-provider';
import { contentInvokes } from './invoke-content';

export const invokeRegistry = Object.freeze({
  ...applicationInvokes,
  ...providerInvokes,
  ...contentInvokes,
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
  'info:update-changed': defineEvent({
    roles: ['main'] as const,
    payload: ApplicationUpdateStateSchema,
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
export type PortTransferRole<Channel extends PortTransferChannel> =
  (typeof portTransferRegistry)[Channel]['roles'][number];
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
