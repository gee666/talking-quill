import { z } from 'zod';
import { HelperRuntimeObservabilitySchema } from '../../shared/helper/protocol';
import {
  HelperReadinessReasonSchema,
  HelperReadinessStatusSchema,
} from '../../shared/schemas/helper-readiness';

export const DiagnosticEventSchema = z.enum([
  'application.started',
  'application.activation',
  'application.stopping',
  'helper.process.started',
  'helper.process.exited',
  'helper.runtime.snapshot',
  'helper.readiness.changed',
  'helper.startup.failure',
  'helper.operational.failure',
  'helper.owner.connection',
]);
export const DiagnosticMetadataSchema = z
  .object({
    component: z.enum(['application', 'helper']).optional(),
    outcome: z
      .union([z.enum(['requested', 'runtime', 'shutdown', 'failure']), HelperReadinessStatusSchema])
      .optional(),
    reason: z.union([HelperReadinessReasonSchema, z.literal('none')]).optional(),
    observability: HelperRuntimeObservabilitySchema.optional(),
    diagnosticId: z.uuid().optional(),
    code: z
      .string()
      .regex(/^[A-Z][A-Z0-9_]{0,63}$/)
      .optional(),
    nativeFailure: z
      .string()
      .regex(/^[a-z0-9][a-z0-9_-]{0,95}$/)
      .optional(),
    appVersion: z
      .string()
      .regex(/^\d+\.\d+\.\d+$/u)
      .optional(),
    runtimeVersion: z
      .string()
      .regex(/^\d+\.\d+\.\d+$/u)
      .optional(),
    exitCode: z.number().int().min(-2_147_483_648).max(4_294_967_295).nullable().optional(),
    exitSignal: z
      .string()
      .regex(/^SIG[A-Z0-9]+$/u)
      .nullable()
      .optional(),
    planned: z.boolean().optional(),
    ownerErrorCategory: z
      .enum([
        'connect',
        'disconnected',
        'uncertain',
        'protocol',
        'acquire_rejected',
        'rejected',
        'sequence_exhausted',
        'transport',
      ])
      .optional(),
    ownerOperation: z
      .string()
      .regex(/^[a-z][a-z._]{0,47}$/u)
      .optional(),
    correlationStatus: z
      .enum([
        'none',
        'not_established',
        'pending',
        'matched',
        'matched_initial_response',
        'mismatched',
        'unexpected_response',
        'unknown',
      ])
      .optional(),
    healthRefresh: z.enum(['not_attempted', 'succeeded', 'failed']).optional(),
    transportStatus: z
      .enum(['open', 'eof', 'closed', 'error', 'backpressured', 'unknown'])
      .optional(),
    ownerProcessState: z.enum(['running', 'exited', 'unknown']).optional(),
    activationSource: z.enum(['second_instance', 'os_activate']).optional(),
    activationSequence: z.number().int().min(1).max(8).optional(),
    restoreHandlerReached: z.boolean().optional(),
    showMainReached: z.boolean().optional(),
  })
  .strict();

export const DiagnosticLogEntrySchema = z
  .object({
    timestamp: z.number().int().nonnegative(),
    event: DiagnosticEventSchema,
    metadata: DiagnosticMetadataSchema,
  })
  .strict();

export type DiagnosticEvent = z.infer<typeof DiagnosticEventSchema>;
export type DiagnosticMetadata = z.infer<typeof DiagnosticMetadataSchema>;
export const DiagnosticFailureCodeSchema = z.enum([
  'HELPER_STARTUP_UNAVAILABLE',
  'HELPER_STARTUP_INCOMPATIBLE',
  'HELPER_RUNTIME_UNAVAILABLE',
  'HELPER_RUNTIME_INCOMPATIBLE',
]);
export type DiagnosticFailureCode = z.infer<typeof DiagnosticFailureCodeSchema>;

export function validateEventMetadata(event: DiagnosticEvent, metadata: DiagnosticMetadata): void {
  const valid =
    (event === 'application.started' &&
      metadata.component === 'application' &&
      metadata.outcome === 'ready' &&
      metadata.observability === undefined &&
      metadata.reason === undefined) ||
    (event === 'application.activation' &&
      metadata.component === 'application' &&
      metadata.outcome === 'requested' &&
      metadata.activationSource !== undefined &&
      metadata.activationSequence !== undefined &&
      metadata.restoreHandlerReached === true &&
      metadata.showMainReached === true) ||
    (event === 'application.stopping' &&
      metadata.component === 'application' &&
      metadata.outcome === 'requested' &&
      metadata.observability === undefined &&
      metadata.reason === undefined) ||
    (event === 'helper.process.started' &&
      metadata.component === 'helper' &&
      metadata.outcome === 'runtime' &&
      metadata.runtimeVersion !== undefined &&
      metadata.exitCode === undefined &&
      metadata.exitSignal === undefined) ||
    (event === 'helper.process.exited' &&
      metadata.component === 'helper' &&
      ((metadata.planned === true && metadata.outcome === 'shutdown') ||
        (metadata.planned === false && metadata.outcome === 'failure')) &&
      metadata.runtimeVersion !== undefined &&
      metadata.exitCode !== undefined &&
      metadata.exitSignal !== undefined) ||
    (event === 'helper.runtime.snapshot' &&
      metadata.component === 'helper' &&
      ['runtime', 'shutdown', 'failure'].includes(metadata.outcome ?? '') &&
      metadata.observability !== undefined &&
      metadata.reason === undefined) ||
    (event === 'helper.owner.connection' &&
      metadata.component === 'helper' &&
      metadata.outcome === 'failure' &&
      metadata.ownerErrorCategory !== undefined &&
      metadata.ownerOperation !== undefined &&
      metadata.correlationStatus !== undefined &&
      metadata.healthRefresh !== undefined &&
      metadata.transportStatus !== undefined &&
      metadata.ownerProcessState !== undefined &&
      metadata.observability === undefined &&
      metadata.reason === undefined) ||
    (event === 'helper.readiness.changed' &&
      metadata.component === 'helper' &&
      HelperReadinessStatusSchema.safeParse(metadata.outcome).success &&
      metadata.reason !== undefined &&
      metadata.observability === undefined &&
      metadata.nativeFailure === undefined) ||
    ((event === 'helper.startup.failure' || event === 'helper.operational.failure') &&
      Object.keys(metadata).length === 1 &&
      DiagnosticFailureCodeSchema.safeParse(metadata.code).success);
  if (!valid) throw new Error('Diagnostic event metadata does not match its event');
}
