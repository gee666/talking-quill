import { mkdir, readFile } from 'node:fs/promises';
import { join } from 'node:path';
import type { SettingsStore } from '../persistence/settings-store';
import { redactSensitive } from './redaction';
import {
  DiagnosticEventSchema,
  DiagnosticMetadataSchema,
  DiagnosticFailureCodeSchema,
  validateEventMetadata,
  type DiagnosticEvent,
  type DiagnosticMetadata,
  type DiagnosticFailureCode,
} from './diagnostic-event-schema';
import { OwnerConnectionAggregateInputSchema } from './owner-aggregate-schema';
import { OwnerDiagnosticAggregate } from './owner-diagnostic-aggregate';
import { DiagnosticFileWriter, isNodeError } from './diagnostic-file-writer';

export {
  DiagnosticEventSchema,
  DiagnosticMetadataSchema,
  DiagnosticLogEntrySchema,
  DiagnosticFailureCodeSchema,
  type DiagnosticEvent,
  type DiagnosticMetadata,
  type DiagnosticFailureCode,
} from './diagnostic-event-schema';
export { OwnerAggregateFileSchema } from './owner-aggregate-schema';

export interface DiagnosticLoggerOptions {
  readonly maxBytes?: number;
  readonly retainedFiles?: number;
  readonly now?: () => number;
  readonly writeOwnerAggregate?: (path: string, contents: Buffer) => Promise<void>;
}

export class DiagnosticLogger {
  readonly #files: DiagnosticFileWriter;
  readonly #ownerAggregate: OwnerDiagnosticAggregate;
  readonly #settings: SettingsStore;
  readonly #directory: string;
  readonly #path: string;
  readonly #maxBytes: number;
  readonly #now: () => number;
  #queuedOperations = 0;
  #queuedOwnerCommits = 0;
  #overflowPersistScheduled = false;
  #enabled = false;
  #initialized = false;
  #disposed = false;
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
    this.#maxBytes = boundedInteger(options.maxBytes ?? 256 * 1024, 1_024, 16 * 1024 * 1024);
    const retainedFiles = boundedInteger(options.retainedFiles ?? 3, 1, 8);
    this.#now = options.now ?? Date.now;
    this.#files = new DiagnosticFileWriter(logsDirectory, this.#maxBytes, retainedFiles, this.#now);
    this.#ownerAggregate = new OwnerDiagnosticAggregate(
      logsDirectory,
      this.#now,
      options.writeOwnerAggregate ??
        (async (path, contents) => {
          await this.#files.atomicReplaceAt(path, contents);
        }),
    );
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
      await this.#ownerAggregate.load();
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
    const task = this.#ownerTail.then(() =>
      this.#ownerAggregate.mergeAndPersistReplay(record, () => this.#enabled && !this.#disposed),
    );
    this.#ownerTail = task
      .then(() => undefined)
      .catch(() => undefined)
      .finally(() => {
        this.#queuedOwnerCommits -= 1;
      });
    return task;
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
      await this.#files.rotateIfNeeded(bytes);
      const existing = await readFile(this.#path).catch((error: unknown) => {
        if (isNodeError(error) && error.code === 'ENOENT') return Buffer.alloc(0);
        throw error;
      });
      await this.#files.atomicReplace(Buffer.concat([existing, Buffer.from(line)]));
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
      this.#ownerAggregate.recordQueueOverflow();
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

  #scheduleOverflowCheckpoint(): void {
    if (!this.#initialized || this.#overflowPersistScheduled || this.#disposed) return;
    this.#overflowPersistScheduled = true;
    const task = this.#ownerTail.then(() => this.#ownerAggregate.persist());
    this.#ownerTail = task
      .catch(() => {
        this.#ownerAggregate.recordPersistenceFailure();
      })
      .finally(() => {
        this.#overflowPersistScheduled = false;
      });
  }
}

function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error('Invalid diagnostic log bound');
  }
  return value;
}
