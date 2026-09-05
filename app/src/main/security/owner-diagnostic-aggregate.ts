import { mkdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import {
  MigrationOnlyLegacyOwnerAggregateFileSchema,
  OwnerAggregateFileSchema,
  type OwnerConnectionAggregateInput,
  type OwnerDimensions,
} from './owner-aggregate-schema';
import { isNodeError } from './diagnostic-file-writer';

/** Durable replay state. The logger owns scheduling and admission to this store. */
export class OwnerDiagnosticAggregate {
  readonly #directory: string;
  readonly #ownerAggregatePath: string;
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

  constructor(
    directory: string,
    now: () => number,
    write: (path: string, contents: Buffer) => Promise<void>,
  ) {
    this.#directory = directory;
    this.#ownerAggregatePath = join(directory, 'owner-connection-counts.json');
    this.#now = now;
    this.#writeOwnerAggregate = write;
  }

  recordQueueOverflow(): void {
    this.#nonOwnerQueueOverflows += 1n;
  }
  recordPersistenceFailure(): void {
    this.#ownerPersistenceFailures += 1n;
  }

  async mergeAndPersistReplay(
    record: OwnerConnectionAggregateInput,
    canPersist: () => boolean,
  ): Promise<boolean> {
    if (!canPersist()) return false;
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
        await this.persist();
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
      await this.persist();
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
      await this.persist();
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
      await this.persist();
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

  async load(): Promise<void> {
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

  async persist(): Promise<void> {
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
}

function maxBigInt(left: bigint, right: bigint): bigint {
  return left > right ? left : right;
}
