import { z } from 'zod';
import { DictationProfileIdSchema } from '../schemas/dictation-profiles';
import { ShortcutSchema, shortcutIdentity, shortcutsConflict } from '../schemas/shortcut';

export const HELPER_PROTOCOL_VERSION = 3 as const;
export const HELPER_MAX_FRAME_BYTES = 16 * 1024;
const HelperNumericRequestIdSchema = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const HelperStringRequestIdSchema = z
  .string()
  .min(1)
  .refine((value) => new TextEncoder().encode(value).byteLength <= 64, {
    message: 'String request IDs must not exceed 64 UTF-8 bytes',
  });
export const HelperRequestIdSchema = z.union([
  HelperNumericRequestIdSchema,
  HelperStringRequestIdSchema,
]);

export const HelperHookStatusSchema = z.enum([
  'ready',
  'permission_required',
  'unavailable',
  'stopped',
]);
export const HelperPermissionStateSchema = z.enum([
  'granted',
  'denied',
  'unknown',
  'not_applicable',
]);
export const HelperPermissionsSchema = z
  .object({
    accessibility: HelperPermissionStateSchema,
    inputMonitoring: HelperPermissionStateSchema,
    eventPost: HelperPermissionStateSchema,
  })
  .strict();

export const HelperInitializeResultSchema = z
  .object({
    protocolVersion: z.literal(HELPER_PROTOCOL_VERSION),
    helperVersion: z.string().regex(/^\d+\.\d+\.\d+$/),
    platform: z.enum(['windows', 'macos']),
    architecture: z.enum(['x86_64', 'aarch64']),
    hookStatus: HelperHookStatusSchema,
    permissions: HelperPermissionsSchema,
  })
  .strict();
export const HelperPasteResultSchema = z.union([
  z.object({ submitted: z.literal(true) }).strict(),
  z
    .object({
      submitted: z.literal(false),
      reason: z.enum([
        'permission_denied',
        'secure_input',
        'conflicting_modifiers',
        'os_rejected',
        'unavailable',
      ]),
    })
    .strict(),
]);
export const HelperWindowBoundsSchema = z
  .object({
    x: z.number().int(),
    y: z.number().int(),
    width: z.number().int().positive(),
    height: z.number().int().positive(),
  })
  .strict();
export const HelperFrontAppSchema = z
  .object({
    processName: z.string().max(7 * 1024),
    windowTitle: z.string().max(7 * 1024),
    windowBounds: HelperWindowBoundsSchema.nullable(),
  })
  .strict();

const emptySchema = z.object({}).strict();
export const ActivationBindingSchema = z
  .object({
    profileId: DictationProfileIdSchema,
    shortcut: ShortcutSchema,
  })
  .strict();
export const ActivationBindingsSchema = z
  .array(ActivationBindingSchema)
  .max(10)
  .superRefine((bindings, context) => {
    const profileIds = new Set<string>();
    const identities = new Set<string>();
    const prior: z.infer<typeof ShortcutSchema>[] = [];
    for (const [index, binding] of bindings.entries()) {
      if (profileIds.has(binding.profileId)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'profileId'],
          message: 'Activation profile IDs must be distinct',
        });
      }
      const identity = shortcutIdentity(binding.shortcut);
      if (identities.has(identity)) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Activation shortcuts must be distinct',
        });
      } else if (prior.some((candidate) => shortcutsConflict(candidate, binding.shortcut))) {
        context.addIssue({
          code: 'custom',
          path: [index, 'shortcut'],
          message: 'Activation shortcuts with the same modifiers must not prefix one another',
        });
      }
      profileIds.add(binding.profileId);
      identities.add(identity);
      prior.push(binding.shortcut);
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
const setCaptureSchema = z.object({ active: z.boolean() }).strict();
const pingResultSchema = z
  .object({ ok: z.literal(true), hookStatus: HelperHookStatusSchema })
  .strict();

export const helperParamsSchemas = Object.freeze({
  initialize: z.object({ protocolVersion: z.literal(HELPER_PROTOCOL_VERSION) }).strict(),
  'activation.configure': configureActivationParamsSchema,
  'session.set_capture': setCaptureSchema,
  'paste.inject': emptySchema,
  'front_app.get': emptySchema,
  'permissions.get': emptySchema,
  ping: emptySchema,
  shutdown: emptySchema,
});

export const helperResultSchemas = Object.freeze({
  initialize: HelperInitializeResultSchema,
  'activation.configure': configureActivationResultSchema,
  'session.set_capture': setCaptureSchema,
  'paste.inject': HelperPasteResultSchema,
  'front_app.get': HelperFrontAppSchema,
  'permissions.get': HelperPermissionsSchema,
  ping: pingResultSchema,
  shutdown: emptySchema,
});

export type HelperMethod = keyof typeof helperParamsSchemas;
export type HelperParams<Method extends HelperMethod> = z.infer<
  (typeof helperParamsSchemas)[Method]
>;
export type HelperResult<Method extends HelperMethod> = z.infer<
  (typeof helperResultSchemas)[Method]
>;
export type ActivationBinding = z.infer<typeof ActivationBindingSchema>;
export type HelperHookStatus = z.infer<typeof HelperHookStatusSchema>;
export type HelperPermissions = z.infer<typeof HelperPermissionsSchema>;
export type HelperInitializeResult = z.infer<typeof HelperInitializeResultSchema>;
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
      params: z
        .object({
          phase: z.enum(['down', 'up']),
          profileId: DictationProfileIdSchema,
          shortcut: ShortcutSchema,
        })
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
]);

export type HelperRpcResponse = z.infer<typeof HelperRpcResponseSchema>;
export type HelperNotification = z.infer<typeof HelperNotificationSchema>;
