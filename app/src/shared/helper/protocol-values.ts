import { z } from 'zod';
import { TRANSCRIPT_MAX_UTF8_BYTES } from '../schemas/transcription';

export const HELPER_PROTOCOL_VERSION = 10 as const;
export const HELPER_MAX_FRAME_BYTES = 16 * 1024;
export const HELPER_MAX_INSERTION_UTF8_BYTES = TRANSCRIPT_MAX_UTF8_BYTES;
const MAX_OWNER_ID_UTF8_BYTES = 128;
export const boundedOwnerIdSchema = z
  .string()
  .min(1)
  .refine((value) => new TextEncoder().encode(value).byteLength <= MAX_OWNER_ID_UTF8_BYTES, {
    message: `Owner identifiers must not exceed ${String(MAX_OWNER_ID_UTF8_BYTES)} UTF-8 bytes`,
  });
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
  'installed_unobserved',
  'physical_observed',
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

export const HelperKeyboardOwnerStateSchema = z.enum([
  'safe_disabled',
  'idle',
  'leased_disabled',
  'leased_enabled',
  'draining',
  'maintenance',
  'degraded',
  'unavailable',
]);
export const HelperKeyboardOwnerSnapshotSchema = z
  .object({
    model: z.literal('out_of_process'),
    protocolVersion: z.literal(1),
    state: HelperKeyboardOwnerStateSchema,
    instanceId: z
      .string()
      .refine(
        (value) =>
          value.length === 0 ||
          new TextEncoder().encode(value).byteLength <= MAX_OWNER_ID_UTF8_BYTES,
        {
          message: `Owner instance IDs must not exceed ${String(MAX_OWNER_ID_UTF8_BYTES)} UTF-8 bytes`,
        },
      ),
    buildId: z
      .string()
      .refine(
        (value) =>
          value.length === 0 ||
          new TextEncoder().encode(value).byteLength <= MAX_OWNER_ID_UTF8_BYTES,
        {
          message: `Owner build IDs must not exceed ${String(MAX_OWNER_ID_UTF8_BYTES)} UTF-8 bytes`,
        },
      ),
    leaseEpoch: z.number().int().min(1).max(Number.MAX_SAFE_INTEGER).nullable(),
    authenticated: z.boolean(),
  })
  .strict()
  .superRefine((owner, context) => {
    if (owner.authenticated) {
      for (const field of ['instanceId', 'buildId'] as const) {
        if (owner[field].length === 0) {
          context.addIssue({
            code: 'custom',
            path: [field],
            message: 'Authenticated owner identity is required',
          });
        }
      }
      if (owner.leaseEpoch === null) {
        context.addIssue({
          code: 'custom',
          path: ['leaseEpoch'],
          message: 'Authenticated owner lease epoch is required',
        });
      }
    } else {
      for (const field of ['instanceId', 'buildId'] as const) {
        if (owner[field].length !== 0) {
          context.addIssue({
            code: 'custom',
            path: [field],
            message: 'Unauthenticated owners cannot expose identity',
          });
        }
      }
      if (owner.leaseEpoch !== null) {
        context.addIssue({
          code: 'custom',
          path: ['leaseEpoch'],
          message: 'Unauthenticated owners cannot expose a lease epoch',
        });
      }
    }
  });

const HelperKeyboardCaptureCapabilitySchema = z
  .object({
    activationAvailable: z.boolean(),
    sessionKeyCaptureAvailable: z.boolean(),
    runtimeRollbackActive: z.boolean(),
    buildDisabled: z.boolean(),
  })
  .strict()
  .refine(
    (capability) =>
      capability.activationAvailable === capability.sessionKeyCaptureAvailable &&
      (!capability.activationAvailable ||
        (!capability.buildDisabled && !capability.runtimeRollbackActive)),
    { message: 'Keyboard capture must be all-or-nothing and respect process rollback gates' },
  );

export const HelperInitializeResultSchema = z
  .object({
    protocolVersion: z.literal(HELPER_PROTOCOL_VERSION),
    helperVersion: z.string().regex(/^\d+\.\d+\.\d+$/),
    platform: z.enum(['windows', 'macos']),
    architecture: z.enum(['x86_64', 'aarch64']),
    hookStatus: HelperHookStatusSchema,
    permissions: HelperPermissionsSchema,
    keyboardCapture: HelperKeyboardCaptureCapabilitySchema,
    keyboardOwner: HelperKeyboardOwnerSnapshotSchema,
  })
  .strict()
  .superRefine((initialized, context) => {
    if (
      initialized.keyboardCapture.activationAvailable &&
      (!initialized.keyboardOwner.authenticated ||
        initialized.keyboardOwner.leaseEpoch === null ||
        initialized.keyboardOwner.state !== 'leased_disabled')
    ) {
      context.addIssue({
        code: 'custom',
        path: ['keyboardCapture', 'activationAvailable'],
        message: 'Keyboard capture initialization requires an authenticated disabled owner lease',
      });
    }
  });
export const HelperActivationGenerationSchema = z
  .number()
  .int()
  .min(1)
  .max(Number.MAX_SAFE_INTEGER);
export const HelperNativeTargetTokenSchema = z
  .string()
  .min(1)
  .refine((value) => new TextEncoder().encode(value).byteLength <= 64, {
    message: 'Native target tokens must not exceed 64 UTF-8 bytes',
  });
export const HelperActivationContextSchema = z
  .object({
    activationGeneration: HelperActivationGenerationSchema,
    targetToken: HelperNativeTargetTokenSchema.nullable(),
  })
  .strict();
export const HelperClipboardSha256Schema = z.string().regex(/^[0-9a-f]{64}$/);
export const HelperDiagnosticIdentitySchema = z
  .string()
  .regex(/^[0-9a-f]{64}$/u)
  .refine((value) => value !== '0'.repeat(64) && value !== 'f'.repeat(64), {
    message: 'Diagnostic identities cannot use retired sentinel values',
  });
export const HelperPasteInjectParamsSchema = HelperActivationContextSchema.extend({
  expectedClipboardSha256: HelperClipboardSha256Schema,
}).strict();

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
        'indeterminate',
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
