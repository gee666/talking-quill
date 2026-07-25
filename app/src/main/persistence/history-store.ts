import { randomUUID } from 'node:crypto';
import { chmodSync, existsSync } from 'node:fs';
import Database from 'better-sqlite3';
import {
  HistoryCreateSchema,
  HistoryCursorSchema,
  HistoryIdSchema,
  HistoryRecordSchema,
  HistoryUpdateSchema,
  type HistoryCreate,
  type HistoryCursor,
  type HistoryRecord,
  type HistoryUpdate,
} from '../../shared/schemas/history';
import {
  TRANSCRIPT_MAX_CHARACTERS,
  TRANSCRIPT_MAX_UTF8_BYTES,
} from '../../shared/schemas/transcription';

const HISTORY_SCHEMA_VERSION = 2;
const MIGRATION_BATCH_SIZE = 256;
const SCREENSHOT_LOOKUP_BATCH_SIZE = 256;
const ScreenshotFilenameSchema = HistoryCreateSchema.shape.screenshotFilename.unwrap();

interface HistoryRow {
  readonly id: string;
  readonly created_at: number;
  readonly dictation_mode: string;
  readonly processing_mode: string;
  readonly outcome: string;
  readonly raw_text: string | null;
  readonly processed_text: string | null;
  readonly provider_id: string | null;
  readonly model_id: string | null;
  readonly fell_back: number;
  readonly error_category: string | null;
  readonly voice_trigger: string | null;
  readonly voice_snippet: string | null;
  readonly screenshot_filename: string | null;
}

export interface HistoryPage {
  readonly items: readonly HistoryRecord[];
  readonly nextCursor: HistoryCursor | null;
}

export class HistoryStore {
  readonly #database: Database.Database;
  #closed = false;

  constructor(path: string) {
    this.#database = new Database(path);
    try {
      this.#database.pragma('foreign_keys = ON');
      if (this.#database.pragma('journal_mode', { simple: true }) !== 'wal') {
        this.#database.pragma('journal_mode = WAL');
      }
      this.#database.pragma('busy_timeout = 5000');
      this.#migrate();
      for (const ownedFile of [path, `${path}-wal`, `${path}-shm`]) {
        if (existsSync(ownedFile)) {
          try {
            chmodSync(ownedFile, 0o600);
          } catch (error: unknown) {
            if (process.platform !== 'win32') throw error;
            // Windows does not expose POSIX file modes; access remains user-scoped.
          }
        }
      }
    } catch (error: unknown) {
      this.#database.close();
      this.#closed = true;
      throw error;
    }
  }

  create(input: HistoryCreate): HistoryRecord {
    this.#assertOpen();
    const parsed = HistoryCreateSchema.parse(input);
    const record = HistoryRecordSchema.parse({
      ...parsed,
      id: randomUUID(),
      createdAt: parsed.createdAt ?? Date.now(),
    });
    this.#database
      .prepare(
        `INSERT INTO history (
          id, created_at, dictation_mode, processing_mode, outcome, raw_text, processed_text,
          provider_id, model_id, fell_back, error_category, voice_trigger, voice_snippet,
          screenshot_filename
        ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
      )
      .run(
        record.id,
        record.createdAt,
        record.dictationMode,
        record.processingMode,
        record.outcome,
        record.rawText,
        record.processedText,
        record.providerId,
        record.modelId,
        record.fellBack ? 1 : 0,
        record.errorCategory,
        record.voiceTrigger,
        record.voiceSnippet,
        record.screenshotFilename,
      );
    return record;
  }

  getById(id: string): HistoryRecord | null {
    this.#assertOpen();
    const parsedId = HistoryIdSchema.parse(id);
    const row = this.#database.prepare('SELECT * FROM history WHERE id = ?').get(parsedId) as
      HistoryRow | undefined;
    return row === undefined ? null : mapRow(row);
  }

  update(id: string, patch: HistoryUpdate): HistoryRecord | null {
    this.#assertOpen();
    const parsedId = HistoryIdSchema.parse(id);
    const parsedPatch = HistoryUpdateSchema.parse(patch);
    return this.#database.transaction(() => {
      const currentRow = this.#database
        .prepare('SELECT * FROM history WHERE id = ?')
        .get(parsedId) as HistoryRow | undefined;
      if (currentRow === undefined) return null;

      const updated = HistoryRecordSchema.parse({ ...mapRow(currentRow), ...parsedPatch });
      this.#database
        .prepare(
          `UPDATE history SET
            dictation_mode = ?, processing_mode = ?, outcome = ?, raw_text = ?,
            processed_text = ?, provider_id = ?, model_id = ?, fell_back = ?,
            error_category = ?, voice_trigger = ?, voice_snippet = ?, screenshot_filename = ?
           WHERE id = ?`,
        )
        .run(
          updated.dictationMode,
          updated.processingMode,
          updated.outcome,
          updated.rawText,
          updated.processedText,
          updated.providerId,
          updated.modelId,
          updated.fellBack ? 1 : 0,
          updated.errorCategory,
          updated.voiceTrigger,
          updated.voiceSnippet,
          updated.screenshotFilename,
          parsedId,
        );
      const result = this.#database.prepare('SELECT * FROM history WHERE id = ?').get(parsedId) as
        HistoryRow | undefined;
      if (result === undefined) throw new Error('Updated history record could not be read');
      return mapRow(result);
    })();
  }

  list(limit = 50, cursor: HistoryCursor | null = null): HistoryPage {
    this.#assertOpen();
    const boundedLimit = Math.min(Math.max(Math.trunc(limit), 1), 200);
    const parsedCursor = cursor === null ? null : HistoryCursorSchema.parse(cursor);
    const rows = (
      parsedCursor === null
        ? this.#database
            .prepare('SELECT * FROM history ORDER BY created_at DESC, id DESC LIMIT ?')
            .all(boundedLimit + 1)
        : this.#database
            .prepare(
              `SELECT * FROM history
               WHERE created_at < ? OR (created_at = ? AND id < ?)
               ORDER BY created_at DESC, id DESC LIMIT ?`,
            )
            .all(parsedCursor.createdAt, parsedCursor.createdAt, parsedCursor.id, boundedLimit + 1)
    ) as HistoryRow[];
    const hasMore = rows.length > boundedLimit;
    const items = rows.slice(0, boundedLimit).map(mapRow);
    const last = items.at(-1);
    return {
      items,
      nextCursor: hasMore && last !== undefined ? { createdAt: last.createdAt, id: last.id } : null,
    };
  }

  deleteById(id: string): boolean {
    this.#assertOpen();
    const parsedId = HistoryIdSchema.parse(id);
    return this.#database.prepare('DELETE FROM history WHERE id = ?').run(parsedId).changes > 0;
  }

  deleteAll(): number {
    this.#assertOpen();
    return this.#database.prepare('DELETE FROM history').run().changes;
  }

  listScreenshotFilenames(): readonly string[] {
    this.#assertOpen();
    const rows = this.#database
      .prepare('SELECT screenshot_filename FROM history WHERE screenshot_filename IS NOT NULL')
      .all() as { readonly screenshot_filename: unknown }[];
    return rows.map((row) => ScreenshotFilenameSchema.parse(row.screenshot_filename));
  }

  listRetainedScreenshotFilenames(candidates: readonly string[]): ReadonlySet<string> {
    this.#assertOpen();
    const unique = [...new Set(candidates.map((value) => ScreenshotFilenameSchema.parse(value)))];
    const retained = new Set<string>();
    for (let offset = 0; offset < unique.length; offset += SCREENSHOT_LOOKUP_BATCH_SIZE) {
      const batch = unique.slice(offset, offset + SCREENSHOT_LOOKUP_BATCH_SIZE);
      const placeholders = batch.map(() => '?').join(', ');
      const rows = this.#database
        .prepare(
          `SELECT DISTINCT screenshot_filename FROM history WHERE screenshot_filename IN (${placeholders})`,
        )
        .all(...batch) as { readonly screenshot_filename: unknown }[];
      for (const row of rows) {
        retained.add(ScreenshotFilenameSchema.parse(row.screenshot_filename));
      }
    }
    return retained;
  }

  deleteOlderThan(cutoffTimestamp: number): number {
    return this.deleteOlderThanWithScreenshots(cutoffTimestamp).deletedCount;
  }

  deleteOlderThanWithScreenshots(cutoffTimestamp: number): {
    readonly deletedCount: number;
    readonly screenshotFilenames: readonly string[];
  } {
    this.#assertOpen();
    const cutoff = HistoryCursorSchema.shape.createdAt.parse(cutoffTimestamp);
    return this.#database.transaction(() => {
      const rows = this.#database
        .prepare(
          `SELECT DISTINCT screenshot_filename FROM history
           WHERE created_at < ? AND screenshot_filename IS NOT NULL`,
        )
        .all(cutoff) as { readonly screenshot_filename: unknown }[];
      const screenshotFilenames = rows.map((row) =>
        ScreenshotFilenameSchema.parse(row.screenshot_filename),
      );
      const deletedCount = this.#database
        .prepare('DELETE FROM history WHERE created_at < ?')
        .run(cutoff).changes;
      return { deletedCount, screenshotFilenames };
    })();
  }

  close(): void {
    if (this.#closed) return;
    this.#database.close();
    this.#closed = true;
  }

  #migrate(): void {
    const current = this.#database.pragma('user_version', { simple: true });
    if (typeof current !== 'number' || current > HISTORY_SCHEMA_VERSION) {
      throw new Error('Unsupported history database version');
    }
    if (current === 0) {
      this.#database.transaction(() => {
        this.#database.exec(`
          CREATE TABLE history (
            id TEXT PRIMARY KEY NOT NULL,
            created_at INTEGER NOT NULL,
            dictation_mode TEXT NOT NULL CHECK (dictation_mode IN ('quick', 'extended')),
            processing_mode TEXT NOT NULL CHECK (processing_mode IN ('raw', 'smart')),
            outcome TEXT NOT NULL CHECK (outcome IN (
              'raw-completed', 'smart-completed', 'smart-fallback', 'voice-command', 'error'
            )),
            raw_text TEXT,
            processed_text TEXT,
            provider_id TEXT,
            model_id TEXT,
            fell_back INTEGER NOT NULL CHECK (fell_back IN (0, 1)),
            error_category TEXT,
            voice_trigger TEXT,
            voice_snippet TEXT,
            screenshot_filename TEXT
          );
          CREATE INDEX history_created_at_idx ON history (created_at DESC, id DESC);
          PRAGMA user_version = 2;
        `);
      })();
      return;
    }
    if (current === 1) {
      this.#database.transaction(() => {
        const selectBatch = this.#database.prepare(
          `SELECT id, raw_text, processed_text, voice_snippet FROM history
           WHERE id > ? ORDER BY id LIMIT ?`,
        );
        const update = this.#database.prepare(
          'UPDATE history SET raw_text = ?, processed_text = ?, voice_snippet = ? WHERE id = ?',
        );
        let lastId = '';
        for (;;) {
          const rows = selectBatch.all(lastId, MIGRATION_BATCH_SIZE) as Pick<
            HistoryRow,
            'id' | 'raw_text' | 'processed_text' | 'voice_snippet'
          >[];
          if (rows.length === 0) break;
          for (const row of rows) {
            update.run(
              truncateLegacyTranscript(row.raw_text),
              truncateLegacyTranscript(row.processed_text),
              truncateLegacyTranscript(row.voice_snippet),
              row.id,
            );
          }
          lastId = rows.at(-1)?.id ?? lastId;
        }
        this.#database.pragma('user_version = 2');
      })();
    }
  }

  #assertOpen(): void {
    if (this.#closed) throw new Error('HistoryStore is closed');
  }
}

function truncateLegacyTranscript(value: string | null): string | null {
  if (value === null) return null;
  const characterBounded = value.slice(0, TRANSCRIPT_MAX_CHARACTERS);
  const encoded = Buffer.from(characterBounded, 'utf8');
  if (encoded.byteLength <= TRANSCRIPT_MAX_UTF8_BYTES) return characterBounded;
  for (let end = TRANSCRIPT_MAX_UTF8_BYTES; end >= TRANSCRIPT_MAX_UTF8_BYTES - 3; end -= 1) {
    try {
      return new TextDecoder('utf-8', { fatal: true }).decode(encoded.subarray(0, end));
    } catch {
      // A UTF-8 scalar is at most four bytes, so one of these boundaries is valid.
    }
  }
  throw new Error('Could not bound a legacy history transcript');
}

function mapRow(row: HistoryRow): HistoryRecord {
  return HistoryRecordSchema.parse({
    id: row.id,
    createdAt: row.created_at,
    dictationMode: row.dictation_mode,
    processingMode: row.processing_mode,
    outcome: row.outcome,
    rawText: row.raw_text,
    processedText: row.processed_text,
    providerId: row.provider_id,
    modelId: row.model_id,
    fellBack: row.fell_back === 1,
    errorCategory: row.error_category,
    voiceTrigger: row.voice_trigger,
    voiceSnippet: row.voice_snippet,
    screenshotFilename: row.screenshot_filename,
  });
}
