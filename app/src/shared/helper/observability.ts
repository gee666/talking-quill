import { z } from 'zod';
import {
  HelperKeyboardOwnerSnapshotSchema,
  HelperDiagnosticIdentitySchema,
} from './protocol-values';

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
export const HelperDiagnosticAckParamsSchema = z
  .object({
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
    dimensions: HelperDiagnosticDimensionsSchema,
    count: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
  })
  .strict();
