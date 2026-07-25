import { z } from 'zod';
import { HelperPermissionsSchema } from '../helper/protocol';

export const HelperReadinessStatusSchema = z.enum([
  'starting',
  'ready',
  'permission-required',
  'unavailable',
  'incompatible',
  'stopped',
]);
export const HelperReadinessReasonSchema = z.enum([
  'binary-missing',
  'binary-invalid',
  'spawn-failed',
  'handshake-timeout',
  'protocol-mismatch',
  'malformed-response',
  'input-monitoring-required',
  'accessibility-required',
  'event-post-required',
  'hook-fault',
  'request-timeout',
  'crash-loop',
  'unexpected-exit',
  'shutdown',
]);
export const HelperReadinessSchema = z
  .object({
    status: HelperReadinessStatusSchema,
    reason: HelperReadinessReasonSchema.nullable(),
    helperVersion: z
      .string()
      .regex(/^\d+\.\d+\.\d+$/)
      .nullable(),
    permissions: HelperPermissionsSchema,
  })
  .strict();

export type HelperReadiness = z.infer<typeof HelperReadinessSchema>;
export type HelperReadinessReason = z.infer<typeof HelperReadinessReasonSchema>;

export const DEFAULT_HELPER_PERMISSIONS = Object.freeze({
  accessibility: 'unknown',
  inputMonitoring: 'unknown',
  eventPost: 'unknown',
} as const);

export const INITIAL_HELPER_READINESS: HelperReadiness = Object.freeze({
  status: 'starting',
  reason: null,
  helperVersion: null,
  permissions: DEFAULT_HELPER_PERMISSIONS,
});
