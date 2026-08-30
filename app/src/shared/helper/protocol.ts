import { z } from 'zod';
import {
  DictationProfileIdSchema,
  MAX_DICTATION_PROFILES,
  isReservedBindingForProfile,
} from '../schemas/dictation-profiles';
import { ShortcutSchema, shortcutIdentity } from '../schemas/shortcut';
import { TRANSCRIPT_MAX_UTF8_BYTES } from '../schemas/transcription';

export const HELPER_PROTOCOL_VERSION = 10 as const;
export const HELPER_MAX_FRAME_BYTES = 16 * 1024;
export const HELPER_MAX_INSERTION_UTF8_BYTES = TRANSCRIPT_MAX_UTF8_BYTES;
const MAX_OWNER_ID_UTF8_BYTES = 128;
const boundedOwnerIdSchema = z
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
const AggregateCounterSchema = z.number().int().min(0).max(Number.MAX_SAFE_INTEGER);
const EffectOutcomeCountersSchema = z
  .object({
    attempted: AggregateCounterSchema,
    succeeded: AggregateCounterSchema,
    partial: AggregateCounterSchema,
    failed: AggregateCounterSchema,
  })
  .strict();
export const HelperOwnerObservabilitySchema = z
  .object({
    starts: AggregateCounterSchema,
    cleanExits: AggregateCounterSchema,
    abnormalExits: AggregateCounterSchema,
    singletonCollisions: AggregateCounterSchema,
    authAttempts: AggregateCounterSchema,
    authFailures: z
      .object({
        crossUser: AggregateCounterSchema,
        wrongSession: AggregateCounterSchema,
        codeIdentity: AggregateCounterSchema,
        mac: AggregateCounterSchema,
        protocol: AggregateCounterSchema,
      })
      .strict(),
    leaseAcquired: AggregateCounterSchema,
    leaseRenewed: AggregateCounterSchema,
    leaseExpired: AggregateCounterSchema,
    leaseDisconnected: AggregateCounterSchema,
    leaseReleasedNeutral: AggregateCounterSchema,
    leaseReleasedDraining: AggregateCounterSchema,
    drainDurationMsTotal: AggregateCounterSchema,
    drainDurationMsMax: AggregateCounterSchema,
    maintenancePostponed: AggregateCounterSchema,
    handoffSucceeded: AggregateCounterSchema,
    handoffFailed: AggregateCounterSchema,
    degraded: AggregateCounterSchema,
    hookRecoveries: AggregateCounterSchema,
  })
  .strict()
  .refine((owner) => owner.drainDurationMsMax <= owner.drainDurationMsTotal, {
    message: 'Maximum owner drain duration cannot exceed total duration',
    path: ['drainDurationMsMax'],
  });
export const HelperRuntimeObservabilitySchema = z
  .object({
    keyboardOwner: HelperKeyboardOwnerSnapshotSchema,
    owner: HelperOwnerObservabilitySchema,
    registeredInput: z
      .object({
        hookInstalled: AggregateCounterSchema,
        pumpAlive: AggregateCounterSchema,
        hcActionCallbacks: AggregateCounterSchema,
        physicalCallbacks: AggregateCounterSchema,
        physicalCallbacksFiltered: AggregateCounterSchema,
        registeredCandidateCallbacks: AggregateCounterSchema,
        registeredMatchCallbacks: AggregateCounterSchema,
        registeredReleaseCallbacks: AggregateCounterSchema,
        callbackChannelAccepted: AggregateCounterSchema,
        callbackChannelRejected: AggregateCounterSchema,
        adapterDequeued: AggregateCounterSchema,
        ownerAdmitted: AggregateCounterSchema,
        ownerFlushed: AggregateCounterSchema,
        ownerRejected: AggregateCounterSchema,
        gatewayReceived: AggregateCounterSchema,
        v10NotificationAccepted: AggregateCounterSchema,
        electronReceived: AggregateCounterSchema,
        observationAccepted: AggregateCounterSchema,
      })
      .strict(),
    keyboardCapture: z
      .object({
        runtimeRollbackActive: z.boolean(),
        developmentDisabled: z.boolean(),
        activationEnableRequestsBlocked: AggregateCounterSchema,
        sessionCaptureRequestsBlocked: AggregateCounterSchema,
        shutdownOwnershipDeadlines: AggregateCounterSchema,
        terminalDisablements: AggregateCounterSchema,
      })
      .strict(),
    transactions: z
      .object({
        started: AggregateCounterSchema,
        committed: AggregateCounterSchema,
        replayed: AggregateCounterSchema,
        cancelled: AggregateCounterSchema,
        journalHighWater: AggregateCounterSchema,
        cancellationReasons: z
          .object({
            invalidContinuation: AggregateCounterSchema,
            modifierChanged: AggregateCounterSchema,
            altGr: AggregateCounterSchema,
            journalOverflow: AggregateCounterSchema,
            configurationReplaced: AggregateCounterSchema,
            revisionMismatch: AggregateCounterSchema,
            gateClosed: AggregateCounterSchema,
            shutdown: AggregateCounterSchema,
            helperDisconnected: AggregateCounterSchema,
            secureDesktop: AggregateCounterSchema,
            timeout: AggregateCounterSchema,
            activationDeliveryFailed: AggregateCounterSchema,
            neutralizationFailed: AggregateCounterSchema,
            replayFailed: AggregateCounterSchema,
            effectProtocolViolation: AggregateCounterSchema,
            physicalStateMismatch: AggregateCounterSchema,
            targetChanged: AggregateCounterSchema,
          })
          .strict(),
      })
      .strict(),
    replay: EffectOutcomeCountersSchema,
    dummy: EffectOutcomeCountersSchema,
    paste: z
      .object({
        attempted: AggregateCounterSchema,
        submitted: AggregateCounterSchema,
        targetValidationFallback: AggregateCounterSchema,
        nativeWaitDurationMsTotal: AggregateCounterSchema,
        nativeWaitDurationMsMax: AggregateCounterSchema,
        modifierTimeouts: AggregateCounterSchema,
        failures: z
          .object({
            permissionDenied: AggregateCounterSchema,
            secureInput: AggregateCounterSchema,
            conflictingModifiers: AggregateCounterSchema,
            osRejected: AggregateCounterSchema,
            unavailable: AggregateCounterSchema,
            indeterminate: AggregateCounterSchema,
          })
          .strict(),
      })
      .strict(),
  })
  .strict()
  .superRefine((value, context) => {
    if (value.paste.nativeWaitDurationMsMax > value.paste.nativeWaitDurationMsTotal) {
      context.addIssue({
        code: 'custom',
        path: ['paste', 'nativeWaitDurationMsMax'],
        message: 'Maximum native modifier wait cannot exceed total wait duration',
      });
    }
  });

export const HelperTerminalObservabilityRecordSchema = z
  .object({
    event: z.literal('helper.runtime.terminal'),
    outcome: z.enum(['shutdown', 'failure']),
    observability: HelperRuntimeObservabilitySchema,
  })
  .strict();

const HelperDiagnosticDimensionsSchema = z
  .object({
    category: z.enum([
      'connect',
      'disconnected',
      'uncertain',
      'protocol',
      'acquire_rejected',
      'rejected',
      'sequence_exhausted',
      'transport',
    ]),
    operation: z.enum([
      'capture.replace_configuration',
      'capture.set_enabled',
      'command.sequence',
      'connect.reconcile',
      'established_operation',
      'front_app.get',
      'front_app.metadata.get',
      'front_app.metadata_get',
      'health.get',
      'lease.acquire',
      'lease.release',
      'lease.renew',
      'observability.get',
      'paste.await_commit',
      'paste.inject',
      'permissions.get',
      'runtime.rollback',
      'service',
      'service.poll',
      'session.reconcile_off',
      'session.set_mode',
    ]),
    correlationStatus: z.enum([
      'none',
      'not_established',
      'pending',
      'matched',
      'matched_initial_response',
      'mismatched',
      'unexpected_response',
      'unknown',
    ]),
    healthRefresh: z.enum(['not_attempted', 'succeeded', 'failed']),
    transportStatus: z.enum(['open', 'eof', 'closed', 'error', 'backpressured', 'unknown']),
    ownerProcessState: z.enum(['running', 'exited', 'unknown']),
  })
  .strict();
const HelperDiagnosticAckParamsSchema = z
  .object({
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
    dimensions: HelperDiagnosticDimensionsSchema,
    count: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
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
