import { z } from 'zod';

export const HELPER_PROTOCOL_VERSION = 2 as const;
export const HELPER_MAX_FRAME_BYTES = 16 * 1024;
export const HelperRequestIdSchema = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);

export const ActivationKeySchema = z.enum([
  'A',
  'B',
  'C',
  'D',
  'E',
  'F',
  'G',
  'H',
  'I',
  'J',
  'K',
  'L',
  'M',
  'N',
  'O',
  'P',
  'Q',
  'R',
  'S',
  'T',
  'U',
  'V',
  'W',
  'X',
  'Y',
  'Z',
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
    defaultActivationKey: ActivationKeySchema,
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
  .object({ key: ActivationKeySchema, shift: z.boolean() })
  .strict();
export const ActivationBindingsSchema = z
  .array(ActivationBindingSchema)
  .max(10)
  .superRefine((bindings, context) => {
    const seen = new Set<string>();
    for (const [index, binding] of bindings.entries()) {
      const exact = `${binding.shift ? 'shift+' : ''}${binding.key}`;
      if (seen.has(exact)) {
        context.addIssue({
          code: 'custom',
          path: [index],
          message: 'Activation bindings must be distinct',
        });
      }
      seen.add(exact);
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
export type ActivationKey = z.infer<typeof ActivationKeySchema>;
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
          key: ActivationKeySchema,
          shift: z.boolean(),
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
