import { chmod, mkdir, rename, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { z } from 'zod';
import type { SettingsStore } from '../persistence/settings-store';
import { redactSensitive } from './redaction';

const DiagnosticEventSchema = z.enum(['application.started', 'application.stopping']);
const DiagnosticMetadataSchema = z
  .object({
    component: z.literal('application').optional(),
    outcome: z.enum(['ready', 'requested']).optional(),
    diagnosticId: z.uuid().optional(),
    code: z
      .string()
      .regex(/^[A-Z][A-Z0-9_]{0,63}$/)
      .optional(),
  })
  .strict();

export type DiagnosticEvent = z.infer<typeof DiagnosticEventSchema>;
export type DiagnosticMetadata = z.infer<typeof DiagnosticMetadataSchema>;

export interface DiagnosticLoggerOptions {
  readonly maxBytes?: number;
  readonly retainedFiles?: number;
  readonly now?: () => number;
}

export class DiagnosticLogger {
  readonly #settings: SettingsStore;
  readonly #directory: string;
  readonly #path: string;
  readonly #maxBytes: number;
  readonly #retainedFiles: number;
  readonly #now: () => number;
  #enabled = false;
  #initialized = false;
  #disposed = false;
  #tail: Promise<void> = Promise.resolve();
  #unsubscribe: (() => void) | null = null;

  constructor(
    settings: SettingsStore,
    logsDirectory: string,
    options: DiagnosticLoggerOptions = {},
  ) {
    this.#settings = settings;
    this.#directory = logsDirectory;
    this.#path = join(logsDirectory, 'diagnostic.jsonl');
    this.#maxBytes = boundedInteger(options.maxBytes ?? 256 * 1024, 1_024, 16 * 1024 * 1024);
    this.#retainedFiles = boundedInteger(options.retainedFiles ?? 3, 1, 8);
    this.#now = options.now ?? Date.now;
  }

  get enabled(): boolean {
    return this.#enabled;
  }

  async initialize(): Promise<void> {
    if (this.#initialized || this.#disposed) return;
    this.#initialized = true;
    this.#enabled = this.#settings.get().privacy.diagnosticLoggingEnabled;
    if (this.#enabled) await mkdir(this.#directory, { recursive: true, mode: 0o700 });
    this.#unsubscribe = this.#settings.subscribe((settings) => {
      this.#enabled = settings.privacy.diagnosticLoggingEnabled;
      if (this.#enabled) {
        void this.#enqueue(() => mkdir(this.#directory, { recursive: true, mode: 0o700 })).catch(
          () => undefined,
        );
      }
    });
  }

  record(event: DiagnosticEvent, metadata: DiagnosticMetadata = {}): Promise<void> {
    const parsedEvent = DiagnosticEventSchema.parse(event);
    const parsedMetadata = DiagnosticMetadataSchema.parse(metadata);
    if (!this.#initialized || this.#disposed || !this.#enabled) return Promise.resolve();
    return this.#enqueue(async () => {
      if (this.#disposed || !this.#enabled) return;
      const entry = {
        timestamp: this.#now(),
        event: parsedEvent,
        metadata: redactSensitive(parsedMetadata),
      };
      const line = `${JSON.stringify(entry)}\n`;
      const bytes = Buffer.byteLength(line);
      await mkdir(this.#directory, { recursive: true, mode: 0o700 });
      await this.#rotateIfNeeded(bytes);
      await writeFile(this.#path, line, { encoding: 'utf8', flag: 'a', mode: 0o600 });
      await chmod(this.#path, 0o600).catch(() => undefined);
    });
  }

  async dispose(): Promise<void> {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#enabled = false;
    this.#unsubscribe?.();
    this.#unsubscribe = null;
    await this.#tail;
  }

  #enqueue(operation: () => Promise<unknown>): Promise<void> {
    const task = this.#tail.then(operation).then(() => undefined);
    this.#tail = task.catch(() => undefined);
    return task;
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

function boundedInteger(value: number, minimum: number, maximum: number): number {
  if (!Number.isInteger(value) || value < minimum || value > maximum) {
    throw new Error('Invalid diagnostic log bound');
  }
  return value;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
