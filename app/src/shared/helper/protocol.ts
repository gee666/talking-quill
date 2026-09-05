import { z } from 'zod';
import {
  DictationProfileIdSchema,
  MAX_DICTATION_PROFILES,
  isReservedBindingForProfile,
} from '../schemas/dictation-profiles';
import { ShortcutSchema, shortcutIdentity } from '../schemas/shortcut';
import {
  HELPER_PROTOCOL_VERSION,
  HelperRequestIdSchema,
  HelperHookStatusSchema,
  HelperPermissionsSchema,
  HelperKeyboardOwnerSnapshotSchema,
  HelperInitializeResultSchema,
  HelperActivationGenerationSchema,
  HelperNativeTargetTokenSchema,
  type HelperActivationContextSchema,
  type HelperClipboardSha256Schema,
  boundedOwnerIdSchema,
  HelperPasteInjectParamsSchema,
  HelperPasteResultSchema,
  HelperFrontAppSchema,
} from './protocol-values';
import {
  HelperRuntimeObservabilitySchema,
  type HelperTerminalObservabilityRecordSchema,
  HelperDiagnosticAckParamsSchema,
} from './observability';

export {
  HELPER_PROTOCOL_VERSION,
  HELPER_MAX_FRAME_BYTES,
  HELPER_MAX_INSERTION_UTF8_BYTES,
  HelperRequestIdSchema,
  HelperHookStatusSchema,
  HelperPermissionStateSchema,
  HelperPermissionsSchema,
  HelperKeyboardOwnerStateSchema,
  HelperKeyboardOwnerSnapshotSchema,
  HelperInitializeResultSchema,
  HelperActivationGenerationSchema,
  HelperNativeTargetTokenSchema,
  HelperActivationContextSchema,
  HelperClipboardSha256Schema,
  HelperDiagnosticIdentitySchema,
  HelperPasteInjectParamsSchema,
  HelperPasteResultSchema,
  HelperWindowBoundsSchema,
  HelperFrontAppSchema,
} from './protocol-values';
export {
  HelperOwnerObservabilitySchema,
  HelperRuntimeObservabilitySchema,
  HelperTerminalObservabilityRecordSchema,
} from './observability';

const emptySchema = z.object({}).strict();
export const ActivationBindingSchema = z
  .object({
    profileId: DictationProfileIdSchema,
    shortcut: ShortcutSchema,
  })
  .strict();
export const ActivationBindingsSchema = z
  .array(ActivationBindingSchema)
  .max(MAX_DICTATION_PROFILES)
  .superRefine((bindings, context) => {
    const profileIds = new Set<string>();
    const identities = new Set<string>();
    for (const [index, binding] of bindings.entries()) {
      if (profileIds.has(binding.profileId)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'profileId'],
          message: 'Activation profile IDs must be distinct',
        });
      }
      if (isReservedBindingForProfile(binding.profileId, binding.shortcut)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'The canonical built-in shortcut family is reserved for its exact owners',
        });
      }
      const identity = shortcutIdentity(binding.shortcut);
      if (identities.has(identity)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Activation shortcuts must be distinct',
        });
      }
      profileIds.add(binding.profileId);
      identities.add(identity);
    }
  });
const configureActivationParamsSchema = z
  .object({ enabled: z.boolean(), bindings: ActivationBindingsSchema })
  .strict()
  .refine((value) => !value.enabled || value.bindings.length > 0, {
    message: 'Enabled activation requires at least one binding',
    path: ['bindings'],
  });
const configureActivationResultSchema = configureActivationParamsSchema;
export const HelperSessionCaptureModeSchema = z.enum(['off', 'recording', 'cancel-only']);
const setCaptureSchema = z.object({ mode: HelperSessionCaptureModeSchema }).strict();
const pingResultSchema = z
  .object({
    ok: z.literal(true),
    hookStatus: HelperHookStatusSchema,
    keyboardOwner: HelperKeyboardOwnerSnapshotSchema,
  })
  .strict();
export const HelperShutdownResultSchema = z
  .object({ ownerDisposition: z.enum(['neutral', 'draining']) })
  .strict();
const maintenanceBaseSchema = z.object({
  transactionId: boundedOwnerIdSchema,
  sourceBuildId: boundedOwnerIdSchema,
});
export const HelperPrepareMaintenanceParamsSchema = z.discriminatedUnion('operation', [
  maintenanceBaseSchema.extend({ operation: z.literal('uninstall') }).strict(),
  maintenanceBaseSchema
    .extend({
      operation: z.enum(['update', 'rollback']),
      targetBuildId: boundedOwnerIdSchema,
      targetOwnerSha256: z.string().regex(/^[0-9a-f]{64}$/),
    })
    .strict(),
]);
export const HelperPrepareMaintenanceResultSchema = z
  .object({
    maintenanceReady: z.literal(true),
    ownerHandoff: z.string().regex(/^[0-9a-f]{64}$/),
  })
  .strict();
export const helperParamsSchemas = Object.freeze({
  initialize: z.object({ protocolVersion: z.literal(HELPER_PROTOCOL_VERSION) }).strict(),
  'activation.configure': configureActivationParamsSchema,
  'session.set_capture': setCaptureSchema,
  'paste.inject': HelperPasteInjectParamsSchema,
  'front_app.get': emptySchema,
  'permissions.get': emptySchema,
  'runtime.observability': emptySchema,
  ping: emptySchema,
  'diagnostic.ack': HelperDiagnosticAckParamsSchema,
  'owner.prepare_maintenance': HelperPrepareMaintenanceParamsSchema,
  shutdown: emptySchema,
});

export const helperResultSchemas = Object.freeze({
  initialize: HelperInitializeResultSchema,
  'activation.configure': configureActivationResultSchema,
  'session.set_capture': setCaptureSchema,
  'paste.inject': HelperPasteResultSchema,
  'front_app.get': HelperFrontAppSchema,
  'permissions.get': HelperPermissionsSchema,
  'runtime.observability': HelperRuntimeObservabilitySchema,
  ping: pingResultSchema,
  'diagnostic.ack': z.object({ acknowledged: z.boolean() }).strict(),
  'owner.prepare_maintenance': HelperPrepareMaintenanceResultSchema,
  shutdown: HelperShutdownResultSchema,
});

export type HelperMethod = keyof typeof helperParamsSchemas;
export type HelperParams<Method extends HelperMethod> = z.infer<
  (typeof helperParamsSchemas)[Method]
>;
export type HelperResult<Method extends HelperMethod> = z.infer<
  (typeof helperResultSchemas)[Method]
>;
export type ActivationBinding = z.infer<typeof ActivationBindingSchema>;
export type HelperActivationContext = z.infer<typeof HelperActivationContextSchema>;
export type HelperNativeTargetToken = z.infer<typeof HelperNativeTargetTokenSchema>;
export type HelperClipboardSha256 = z.infer<typeof HelperClipboardSha256Schema>;
export type HelperHookStatus = z.infer<typeof HelperHookStatusSchema>;
export type HelperSessionCaptureMode = z.infer<typeof HelperSessionCaptureModeSchema>;
export type HelperPermissions = z.infer<typeof HelperPermissionsSchema>;
export type HelperInitializeResult = z.infer<typeof HelperInitializeResultSchema>;
export type HelperKeyboardOwnerSnapshot = z.infer<typeof HelperKeyboardOwnerSnapshotSchema>;
export type HelperPrepareMaintenanceParams = z.infer<typeof HelperPrepareMaintenanceParamsSchema>;
export type HelperRuntimeObservability = z.infer<typeof HelperRuntimeObservabilitySchema>;
export type HelperTerminalObservabilityRecord = z.infer<
  typeof HelperTerminalObservabilityRecordSchema
>;
export type HelperPasteResult = z.infer<typeof HelperPasteResultSchema>;
export type HelperFrontApp = z.infer<typeof HelperFrontAppSchema>;

export const HelperRpcErrorSchema = z
  .object({
    code: z.number().int().min(-32_768).max(-32_000),
    message: z.string().min(1).max(80),
  })
  .strict();
export const HelperRpcResponseSchema = z.union([
  z
    .object({
      jsonrpc: z.literal('2.0'),
      id: HelperRequestIdSchema.nullable(),
      result: z.unknown(),
    })
    .strict(),
  z
    .object({
      jsonrpc: z.literal('2.0'),
      id: HelperRequestIdSchema.nullable(),
      error: HelperRpcErrorSchema,
    })
    .strict(),
]);

export const HelperNotificationSchema = z.discriminatedUnion('method', [
  z
    .object({
      jsonrpc: z.literal('2.0'),
      method: z.literal('activation.event'),
      params: z.discriminatedUnion('phase', [
        z
          .object({
            phase: z.enum(['down', 'up']),
            profileId: DictationProfileIdSchema,
            shortcut: ShortcutSchema,
            activationGeneration: HelperActivationGenerationSchema,
            targetToken: HelperNativeTargetTokenSchema.nullable(),
          })
          .strict(),
        z
          .object({
            phase: z.literal('complete'),
            profileId: DictationProfileIdSchema,
            shortcut: ShortcutSchema,
            activationGeneration: HelperActivationGenerationSchema,
            targetToken: HelperNativeTargetTokenSchema.nullable(),
            heldMs: z.number().int().nonnegative().max(Number.MAX_SAFE_INTEGER),
          })
          .strict(),
      ]),
    })
    .strict(),
  z
    .object({
      jsonrpc: z.literal('2.0'),
      method: z.literal('registered_input.observed'),
      params: z
        .object({ generation: z.number().int().positive().max(Number.MAX_SAFE_INTEGER) })
        .strict(),
    })
    .strict(),
  z
    .object({
      jsonrpc: z.literal('2.0'),
      method: z.literal('paste.committed'),
      params: z.object({ requestId: HelperRequestIdSchema }).strict(),
    })
    .strict(),
  z
    .object({
      jsonrpc: z.literal('2.0'),
      method: z.literal('session.key'),
      params: z
        .object({ key: z.enum(['escape', 'enter']), phase: z.enum(['down', 'up']) })
        .strict(),
    })
    .strict(),
  z
    .object({
      jsonrpc: z.literal('2.0'),
      method: z.literal('audio.input_devices_changed'),
      params: emptySchema,
    })
    .strict(),
]);

export type HelperRpcResponse = z.infer<typeof HelperRpcResponseSchema>;
export type HelperNotification = z.infer<typeof HelperNotificationSchema>;
