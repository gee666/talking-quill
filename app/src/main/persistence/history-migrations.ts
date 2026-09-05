import type Database from 'better-sqlite3';
import type { HistoryRow } from './history-row';
import {
  TRANSCRIPT_MAX_CHARACTERS,
  TRANSCRIPT_MAX_UTF8_BYTES,
} from '../../shared/schemas/transcription';

const HISTORY_SCHEMA_VERSION = 2;
const MIGRATION_BATCH_SIZE = 256;

export function migrateHistory(database: Database.Database): void {
  const current = database.pragma('user_version', { simple: true });
  if (typeof current !== 'number' || current > HISTORY_SCHEMA_VERSION) {
    throw new Error('Unsupported history database version');
  }
  if (current === 0) {
    database.transaction(() => {
      database.exec(`
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
    database.transaction(() => {
      const selectBatch = database.prepare(
        `SELECT id, raw_text, processed_text, voice_snippet FROM history
         WHERE id > ? ORDER BY id LIMIT ?`,
      );
      const update = database.prepare(
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
      database.pragma('user_version = 2');
    })();
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
