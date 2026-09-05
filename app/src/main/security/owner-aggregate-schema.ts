import { z } from 'zod';
import { HelperDiagnosticIdentitySchema } from '../../shared/helper/protocol';

const OwnerDiagnosticOperationSchema = z.enum([
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
]);
export const OwnerConnectionAggregateInputSchema = z
  .object({
    event: z.literal('helper.owner.connection.replay'),
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
    streamId: HelperDiagnosticIdentitySchema,
    processGeneration: z.string().regex(/^[1-9][0-9]{0,19}$/u),
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
    operation: OwnerDiagnosticOperationSchema,
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
    count: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    counterOverflow: z.boolean(),
    durable: z.boolean(),
    durabilityFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    writerStartFailures: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
    synchronizationRecoveries: z.string().regex(/^(?:0|[1-9][0-9]{0,38})$/u),
  })
  .strict();
export type OwnerConnectionAggregateInput = z.infer<typeof OwnerConnectionAggregateInputSchema>;
export type OwnerDimensions = Omit<
  OwnerConnectionAggregateInput,
  | 'event'
  | 'journalId'
  | 'journalNonce'
  | 'streamId'
  | 'processGeneration'
  | 'count'
  | 'counterOverflow'
  | 'durable'
  | 'durabilityFailures'
  | 'writerStartFailures'
  | 'synchronizationRecoveries'
>;
const OwnerDimensionKeySchema = z
  .string()
  .max(512)
  .refine((value) => {
    try {
      const parsed = JSON.parse(value) as Record<string, unknown>;
      return (
        Object.keys(parsed).sort().join(',') ===
          [
            'category',
            'correlationStatus',
            'healthRefresh',
            'operation',
            'ownerProcessState',
            'transportStatus',
          ].join(',') &&
        OwnerConnectionAggregateInputSchema.safeParse({
          event: 'helper.owner.connection.replay',
          journalId: '01'.repeat(32),
          journalNonce: '23'.repeat(32),
          streamId: '45'.repeat(32),
          processGeneration: '1',
          count: '1',
          counterOverflow: false,
          durable: true,
          durabilityFailures: '0',
          writerStartFailures: '0',
          synchronizationRecoveries: '0',
          ...parsed,
        }).success
      );
    } catch {
      return false;
    }
  });
const DecimalCounterSchema = z.string().regex(/^(?:0|[1-9][0-9]*)$/u);
const OwnerJournalIdentitySchema = z
  .object({
    journalId: HelperDiagnosticIdentitySchema,
    journalNonce: HelperDiagnosticIdentitySchema,
  })
  .strict();
// Version 1 has aggregate totals but no journal identity. This schema is used only
// while loading that exact historical file version and cannot parse live replay or ACK data.
export const MigrationOnlyLegacyOwnerAggregateFileSchema = z
  .object({
    version: z.literal(1),
    updatedAt: z.number().int().nonnegative(),
    acceptedDisconnects: DecimalCounterSchema,
    duplicateCumulativeRecords: DecimalCounterSchema,
    persistenceFailures: DecimalCounterSchema,
    nonOwnerQueueOverflows: DecimalCounterSchema,
    helperCounterOverflowDimensions: z.array(OwnerDimensionKeySchema).max(100_000),
    dimensions: z
      .array(z.object({ key: OwnerDimensionKeySchema, value: DecimalCounterSchema }).strict())
      .max(100_000),
  })
  .strict();
export const OwnerAggregateFileSchema = z
  .object({
    version: z.literal(2),
    updatedAt: z.number().int().nonnegative(),
    acceptedDisconnects: DecimalCounterSchema,
    duplicateCumulativeRecords: DecimalCounterSchema,
    persistenceFailures: DecimalCounterSchema,
    nonOwnerQueueOverflows: DecimalCounterSchema,
    journalCapacityRejects: DecimalCounterSchema,
    streamIdentityCollisions: DecimalCounterSchema,
    helperCounterOverflowDimensions: z.array(OwnerDimensionKeySchema).max(100_000),
    dimensions: z
      .array(z.object({ key: OwnerDimensionKeySchema, value: DecimalCounterSchema }).strict())
      .max(100_000),
    journals: z
      .array(
        OwnerJournalIdentitySchema.extend({
          durabilityFailures: DecimalCounterSchema,
          writerStartFailures: DecimalCounterSchema,
          synchronizationRecoveries: DecimalCounterSchema,
          highWater: z
            .array(z.object({ key: OwnerDimensionKeySchema, value: DecimalCounterSchema }).strict())
            .max(100_000),
        }).strict(),
      )
      .max(32),
    streams: z
      .array(
        OwnerJournalIdentitySchema.extend({
          processGeneration: z.string().regex(/^[1-9][0-9]{0,19}$/u),
          streamId: HelperDiagnosticIdentitySchema,
          lastSeen: z.number().int().nonnegative(),
        }).strict(),
      )
      .max(256),
  })
  .strict();
