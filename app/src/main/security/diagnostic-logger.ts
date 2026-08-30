import { chmod, mkdir, open, readFile, rename, rm, stat } from 'node:fs/promises';
import { join } from 'node:path';
import { z } from 'zod';
import {
  HelperDiagnosticIdentitySchema,
  HelperRuntimeObservabilitySchema,
} from '../../shared/helper/protocol';
import {
  HelperReadinessReasonSchema,
  HelperReadinessStatusSchema,
} from '../../shared/schemas/helper-readiness';
import type { SettingsStore } from '../persistence/settings-store';
import { redactSensitive } from './redaction';

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
const OwnerConnectionAggregateInputSchema = z
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
type OwnerConnectionAggregateInput = z.infer<typeof OwnerConnectionAggregateInputSchema>;
type OwnerDimensions = Omit<
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
const MigrationOnlyLegacyOwnerAggregateFileSchema = z
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

export interface DiagnosticLoggerOptions {
  readonly maxBytes?: number;
  readonly retainedFiles?: number;
  readonly now?: () => number;
  readonly writeOwnerAggregate?: (path: string, contents: Buffer) => Promise<void>;
}

export class DiagnosticLogger {
  readonly #settings: SettingsStore;
  readonly #directory: string;
  readonly #path: string;
  readonly #ownerAggregatePath: string;
  readonly #maxBytes: number;
  readonly #retainedFiles: number;
  readonly #now: () => number;
  readonly #writeOwnerAggregate: (path: string, contents: Buffer) => Promise<void>;
  readonly #ownerTotals = new Map<string, bigint>();
  readonly #ownerJournals = new Map<
    string,
    {
      readonly journalId: string;
      readonly journalNonce: string;
      readonly highWater: Map<string, bigint>;
      durabilityFailures: bigint;
      writerStartFailures: bigint;
      synchronizationRecoveries: bigint;
    }
  >();
  readonly #ownerStreams = new Map<
    string,
    {
      readonly journalId: string;
      readonly journalNonce: string;
      readonly processGeneration: string;
      readonly streamId: string;
      lastSeen: number;
    }
  >();
  readonly #ownerCounterOverflowDimensions = new Set<string>();
  #ownerAcceptedDisconnects = 0n;
  #ownerDuplicateCumulativeRecords = 0n;
  #ownerPersistenceFailures = 0n;
  #ownerJournalCapacityRejects = 0n;
  #ownerStreamIdentityCollisions = 0n;
  #nonOwnerQueueOverflows = 0n;
  #queuedOperations = 0;
  #queuedOwnerCommits = 0;
  #overflowPersistScheduled = false;
  #enabled = false;
  #initialized = false;
  #disposed = false;
  #temporarySequence = 0;
  #tail: Promise<void> = Promise.resolve();
  #ownerTail: Promise<void> = Promise.resolve();
  #unsubscribe: (() => void) | null = null;
  #initializationGeneration = 0;

  constructor(
    settings: SettingsStore,
    logsDirectory: string,
    options: DiagnosticLoggerOptions = {},
  ) {
    this.#settings = settings;
    this.#directory = logsDirectory;
    this.#path = join(logsDirectory, 'diagnostic.jsonl');
    this.#ownerAggregatePath = join(logsDirectory, 'owner-connection-counts.json');
    this.#maxBytes = boundedInteger(options.maxBytes ?? 256 * 1024, 1_024, 16 * 1024 * 1024);
    this.#retainedFiles = boundedInteger(options.retainedFiles ?? 3, 1, 8);
    this.#now = options.now ?? Date.now;
    this.#writeOwnerAggregate =
      options.writeOwnerAggregate ??
      (async (path, contents) => {
        await this.#atomicReplaceAt(path, contents);
      });
  }

  get enabled(): boolean {
    return this.#enabled;
  }

  async initialize(): Promise<void> {
    if (this.#initialized || this.#disposed) return;
    const generation = ++this.#initializationGeneration;
    this.#enabled = this.#settings.get().privacy.diagnosticLoggingEnabled;
    try {
      this.#unsubscribe = this.#settings.subscribe((settings) => {
        this.#enabled = settings.privacy.diagnosticLoggingEnabled;
        if (this.#enabled) {
          void this.#enqueue(() => mkdir(this.#directory, { recursive: true, mode: 0o700 })).catch(
            () => undefined,
          );
        }
      });
      await this.#loadOwnerAggregate();
      if (this.#enabled) await mkdir(this.#directory, { recursive: true, mode: 0o700 });
      if (!this.#initializationIsCurrent(generation)) return;
      this.#initialized = true;
    } catch (error: unknown) {
      this.#unsubscribe?.();
      this.#unsubscribe = null;
      this.#enabled = false;
      this.#initialized = false;
      throw error;
    }
  }

  async initializeBestEffort(timeoutMs = 250): Promise<boolean> {
    const initialization = this.initialize().then(
      () => this.#initialized,
      () => false,
    );
    let timer: NodeJS.Timeout | undefined;
    const completed = await Promise.race([
      initialization,
      new Promise<false>((resolve) => {
        timer = setTimeout(() => resolve(false), Math.max(1, timeoutMs));
      }),
    ]);
    if (timer !== undefined) clearTimeout(timer);
    if (!completed) {
      this.#initializationGeneration += 1;
      this.#unsubscribe?.();
      this.#unsubscribe = null;
      this.#initialized = false;
      this.#enabled = false;
    }
    return completed;
  }

  #initializationIsCurrent(generation: number): boolean {
    return generation === this.#initializationGeneration && !this.#disposed;
  }

  record(event: DiagnosticEvent, metadata: DiagnosticMetadata = {}): Promise<boolean> {
    return this.#record(event, metadata, false);
  }

  /** Writes one documented startup failure code even when detailed diagnostics are off. */
  recordStartupFailure(code: DiagnosticFailureCode): Promise<boolean> {
    return this.#record(
      'helper.startup.failure',
      { code: DiagnosticFailureCodeSchema.parse(code) },
      true,
    );
  }

  /** Writes one documented runtime failure code even when detailed diagnostics are off. */
  recordOperationalFailure(code: DiagnosticFailureCode): Promise<boolean> {
    return this.#record(
      'helper.operational.failure',
      { code: DiagnosticFailureCodeSchema.parse(code) },
      true,
    );
  }

  /** Commits one durable helper replay before HelperClient sends its ACK. */
  recordOwnerConnectionReplay(input: unknown): Promise<boolean> {
    const record = OwnerConnectionAggregateInputSchema.parse(input);
    if (!this.#initialized || !this.#enabled || this.#disposed || this.#queuedOwnerCommits >= 64) {
      return Promise.resolve(false);
    }
    this.#queuedOwnerCommits += 1;
    const task = this.#ownerTail.then(() => this.#mergeAndPersistOwnerReplay(record));
    this.#ownerTail = task
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => {
        this.#queuedOwnerCommits -= 1;
      });
    return task;
  }

  async #mergeAndPersistOwnerReplay(record: OwnerConnectionAggregateInput): Promise<boolean> {
    if (!this.#enabled || this.#disposed) return false;
    const dimensions: OwnerDimensions = {
      category: record.category,
      operation: record.operation,
      correlationStatus: record.correlationStatus,
      healthRefresh: record.healthRefresh,
      transportStatus: record.transportStatus,
      ownerProcessState: record.ownerProcessState,
    };
    const dimensionKey = JSON.stringify(dimensions);
    const journalKey = `${record.journalId}:${record.journalNonce}`;
    let journal = this.#ownerJournals.get(journalKey);
    if (journal === undefined) {
      if (this.#ownerJournals.size >= 32) {
        this.#ownerJournalCapacityRejects += 1n;
        await this.#persistOwnerAggregate();
        return false;
      }
      journal = {
        journalId: record.journalId,
        journalNonce: record.journalNonce,
        highWater: new Map(),
        durabilityFailures: 0n,
        writerStartFailures: 0n,
        synchronizationRecoveries: 0n,
      };
      this.#ownerJournals.set(journalKey, journal);
    }

    const generationKey = `${journalKey}:${record.processGeneration}`;
    const existingStream = this.#ownerStreams.get(generationKey);
    if (existingStream !== undefined && existingStream.streamId !== record.streamId) {
      this.#ownerStreamIdentityCollisions += 1n;
      await this.#persistOwnerAggregate();
      return false;
    }
    this.#ownerStreams.set(generationKey, {
      journalId: record.journalId,
      journalNonce: record.journalNonce,
      processGeneration: record.processGeneration,
      streamId: record.streamId,
      lastSeen: this.#now(),
    });
    this.#collectOldestStreamMetadata();

    journal.durabilityFailures = maxBigInt(
      journal.durabilityFailures,
      BigInt(record.durabilityFailures),
    );
    journal.writerStartFailures = maxBigInt(
      journal.writerStartFailures,
      BigInt(record.writerStartFailures),
    );
    journal.synchronizationRecoveries = maxBigInt(
      journal.synchronizationRecoveries,
      BigInt(record.synchronizationRecoveries),
    );
    if (!record.durable) {
      await this.#persistOwnerAggregate();
      return false;
    }

    const cumulative = BigInt(record.count);
    const previous = journal.highWater.get(dimensionKey) ?? 0n;
    if (cumulative <= previous) {
      this.#ownerDuplicateCumulativeRecords += 1n;
    } else {
      const delta = cumulative - previous;
      journal.highWater.set(dimensionKey, cumulative);
      this.#ownerTotals.set(dimensionKey, (this.#ownerTotals.get(dimensionKey) ?? 0n) + delta);
      this.#ownerAcceptedDisconnects += delta;
    }
    if (record.counterOverflow) this.#ownerCounterOverflowDimensions.add(dimensionKey);
    try {
      await this.#persistOwnerAggregate();
      return true;
    } catch (error: unknown) {
      this.#ownerPersistenceFailures += 1n;
      throw error;
    }
  }

  #collectOldestStreamMetadata(): void {
    while (this.#ownerStreams.size > 256) {
      let oldestKey: string | null = null;
      let oldestSeen = Number.POSITIVE_INFINITY;
      for (const [key, stream] of this.#ownerStreams) {
        if (stream.lastSeen < oldestSeen) {
          oldestKey = key;
          oldestSeen = stream.lastSeen;
        }
      }
      if (oldestKey === null) return;
      this.#ownerStreams.delete(oldestKey);
    }
  }

  #record(
    event: DiagnosticEvent,
    metadata: DiagnosticMetadata,
    persistWhenDisabled: boolean,
  ): Promise<boolean> {
    const parsedEvent = DiagnosticEventSchema.parse(event);
    const parsedMetadata = DiagnosticMetadataSchema.parse(metadata);
    validateEventMetadata(parsedEvent, parsedMetadata);
    if (this.#disposed || (!persistWhenDisabled && (!this.#initialized || !this.#enabled))) {
      return Promise.resolve(false);
    }
    return this.#enqueue(async () => {
      if (!persistWhenDisabled && (this.#disposed || !this.#enabled)) return false;
      const redactedMetadata = redactSensitive(parsedMetadata) as DiagnosticMetadata;
      const entry = {
        timestamp: this.#now(),
        event: parsedEvent,
        metadata:
          parsedEvent === 'helper.owner.connection' && parsedMetadata.ownerOperation !== undefined
            ? { ...redactedMetadata, ownerOperation: parsedMetadata.ownerOperation }
            : redactedMetadata,
      };
      const line = `${JSON.stringify(entry)}\n`;
      const bytes = Buffer.byteLength(line);
      if (bytes > this.#maxBytes) throw new Error('Diagnostic entry exceeds the file cap');
      await mkdir(this.#directory, { recursive: true, mode: 0o700 });
      await this.#rotateIfNeeded(bytes);
      const existing = await readFile(this.#path).catch((error: unknown) => {
        if (isNodeError(error) && error.code === 'ENOENT') return Buffer.alloc(0);
        throw error;
      });
      await this.#atomicReplace(Buffer.concat([existing, Buffer.from(line)]));
      return true;
    });
  }

  async dispose(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#enabled = false;
    this.#unsubscribe?.();
    this.#unsubscribe = null;
    await Promise.all([this.#tail, this.#ownerTail]);
  }

  async disposeBestEffort(timeoutMs = 250): Promise<void> {
    const disposal = this.dispose().catch(() => undefined);
    let timer: NodeJS.Timeout | undefined;
    await Promise.race([
      disposal,
      new Promise<void>((resolve) => {
        timer = setTimeout(resolve, Math.max(1, timeoutMs));
      }),
    ]);
    if (timer !== undefined) clearTimeout(timer);
  }

  #enqueue<T>(operation: () => Promise<T>): Promise<T> {
    if (this.#queuedOperations >= 256) {
      this.#nonOwnerQueueOverflows += 1n;
      this.#scheduleOverflowCheckpoint();
      return Promise.reject(new Error('Diagnostic operation queue is full'));
    }
    this.#queuedOperations += 1;
    const task = this.#tail.then(operation);
    this.#tail = task
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => {
        this.#queuedOperations -= 1;
      });
    return task;
  }

  async #atomicReplace(contents: Buffer): Promise<void> {
    await this.#atomicReplaceAt(this.#path, contents);
  }

  async #atomicReplaceAt(target: string, contents: Buffer): Promise<void> {
    const temporary = join(
      this.#directory,
      `.diagnostic-${String(process.pid)}-${String(this.#now())}-${String((this.#temporarySequence += 1))}.tmp`,
    );
    let handle: Awaited<ReturnType<typeof open>> | null = null;
    try {
      handle = await open(temporary, 'wx', 0o600);
      await handle.writeFile(contents);
      await handle.sync();
      await handle.close();
      handle = null;
      await rename(temporary, target);
      const committed = await open(target, 'r+');
      try {
        await committed.sync();
      } finally {
        await committed.close();
      }
      await chmod(target, 0o600).catch(() => undefined);
    } finally {
      await handle?.close().catch(() => undefined);
      await rm(temporary, { force: true }).catch(() => undefined);
    }
  }

  async #loadOwnerAggregate(): Promise<void> {
    const contents = await readFile(this.#ownerAggregatePath, 'utf8').catch((error: unknown) => {
      if (isNodeError(error) && error.code === 'ENOENT') return null;
      throw error;
    });
    if (contents === null) return;
    const candidate = JSON.parse(contents) as { version?: unknown };
    if (candidate.version === 1) {
      const legacy = MigrationOnlyLegacyOwnerAggregateFileSchema.parse(candidate);
      this.#ownerAcceptedDisconnects = BigInt(legacy.acceptedDisconnects);
      this.#ownerDuplicateCumulativeRecords = BigInt(legacy.duplicateCumulativeRecords);
      this.#ownerPersistenceFailures = BigInt(legacy.persistenceFailures);
      this.#nonOwnerQueueOverflows = BigInt(legacy.nonOwnerQueueOverflows);
      for (const entry of legacy.dimensions) this.#ownerTotals.set(entry.key, BigInt(entry.value));
      for (const key of legacy.helperCounterOverflowDimensions)
        this.#ownerCounterOverflowDimensions.add(key);
      return;
    }
    const checkpoint = OwnerAggregateFileSchema.parse(candidate);
    this.#ownerAcceptedDisconnects = BigInt(checkpoint.acceptedDisconnects);
    this.#ownerDuplicateCumulativeRecords = BigInt(checkpoint.duplicateCumulativeRecords);
    this.#ownerPersistenceFailures = BigInt(checkpoint.persistenceFailures);
    this.#nonOwnerQueueOverflows = BigInt(checkpoint.nonOwnerQueueOverflows);
    this.#ownerJournalCapacityRejects = BigInt(checkpoint.journalCapacityRejects);
    this.#ownerStreamIdentityCollisions = BigInt(checkpoint.streamIdentityCollisions);
    for (const entry of checkpoint.dimensions)
      this.#ownerTotals.set(entry.key, BigInt(entry.value));
    for (const saved of checkpoint.journals) {
      this.#ownerJournals.set(`${saved.journalId}:${saved.journalNonce}`, {
        journalId: saved.journalId,
        journalNonce: saved.journalNonce,
        highWater: new Map(saved.highWater.map((entry) => [entry.key, BigInt(entry.value)])),
        durabilityFailures: BigInt(saved.durabilityFailures),
        writerStartFailures: BigInt(saved.writerStartFailures),
        synchronizationRecoveries: BigInt(saved.synchronizationRecoveries),
      });
    }
    for (const stream of checkpoint.streams) {
      this.#ownerStreams.set(
        `${stream.journalId}:${stream.journalNonce}:${stream.processGeneration}`,
        { ...stream },
      );
    }
    for (const key of checkpoint.helperCounterOverflowDimensions) {
      this.#ownerCounterOverflowDimensions.add(key);
    }
  }

  #scheduleOverflowCheckpoint(): void {
    if (!this.#initialized || this.#overflowPersistScheduled || this.#disposed) return;
    this.#overflowPersistScheduled = true;
    const task = this.#ownerTail.then(() => this.#persistOwnerAggregate());
    this.#ownerTail = task
      .catch(() => {
        this.#ownerPersistenceFailures += 1n;
      })
      .finally(() => {
        this.#overflowPersistScheduled = false;
      });
  }

  async #persistOwnerAggregate(): Promise<void> {
    const checkpoint = OwnerAggregateFileSchema.parse({
      version: 2,
      updatedAt: this.#now(),
      acceptedDisconnects: this.#ownerAcceptedDisconnects.toString(),
      duplicateCumulativeRecords: this.#ownerDuplicateCumulativeRecords.toString(),
      persistenceFailures: this.#ownerPersistenceFailures.toString(),
      nonOwnerQueueOverflows: this.#nonOwnerQueueOverflows.toString(),
      journalCapacityRejects: this.#ownerJournalCapacityRejects.toString(),
      streamIdentityCollisions: this.#ownerStreamIdentityCollisions.toString(),
      helperCounterOverflowDimensions: [...this.#ownerCounterOverflowDimensions].sort(),
      dimensions: [...this.#ownerTotals.entries()]
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, value]) => ({ key, value: value.toString() })),
      journals: [...this.#ownerJournals.values()]
        .sort((left, right) =>
          `${left.journalId}:${left.journalNonce}`.localeCompare(
            `${right.journalId}:${right.journalNonce}`,
          ),
        )
        .map((journal) => ({
          journalId: journal.journalId,
          journalNonce: journal.journalNonce,
          durabilityFailures: journal.durabilityFailures.toString(),
          writerStartFailures: journal.writerStartFailures.toString(),
          synchronizationRecoveries: journal.synchronizationRecoveries.toString(),
          highWater: [...journal.highWater.entries()]
            .sort(([left], [right]) => left.localeCompare(right))
            .map(([key, value]) => ({ key, value: value.toString() })),
        })),
      streams: [...this.#ownerStreams.values()].sort((left, right) =>
        `${left.journalId}:${left.journalNonce}:${left.processGeneration}`.localeCompare(
          `${right.journalId}:${right.journalNonce}:${right.processGeneration}`,
        ),
      ),
    });
    const contents = Buffer.from(`${JSON.stringify(checkpoint)}\n`);
    if (contents.length > 32 * 1024 * 1024) {
      throw new Error('Owner diagnostic aggregate exceeds its finite schema bound');
    }
    await mkdir(this.#directory, { recursive: true, mode: 0o700 });
    await this.#writeOwnerAggregate(this.#ownerAggregatePath, contents);
  }

  async #rotateIfNeeded(incomingBytes: number): Promise<void> {
    const currentBytes = await stat(this.#path).then(
      (value) => value.size,
      (error: unknown) => {
        if (isNodeError(error) && error.code === 'ENOENT') return 0;
        throw error;
      },
    );
    if (currentBytes + incomingBytes <= this.#maxBytes) return;
    await rm(`${this.#path}.${String(this.#retainedFiles)}`, { force: true });
    for (let index = this.#retainedFiles - 1; index >= 1; index -= 1) {
      await rename(`${this.#path}.${String(index)}`, `${this.#path}.${String(index + 1)}`).catch(
        (error: unknown) => {
          if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
        },
      );
    }
    await rename(this.#path, `${this.#path}.1`).catch((error: unknown) => {
      if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    });
  }
}

function validateEventMetadata(event: DiagnosticEvent, metadata: DiagnosticMetadata): void {
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

function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error('Invalid diagnostic log bound');
  }
  return value;
}

function maxBigInt(left: bigint, right: bigint): bigint {
  return left > right ? left : right;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
