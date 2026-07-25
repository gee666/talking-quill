import { randomUUID } from 'node:crypto';
import { createRequire } from 'node:module';
import { join } from 'node:path';
import type BetterSqlite3 from 'better-sqlite3';
import { afterEach, describe, expect, it } from 'vitest';
import { HistoryStore } from '../../app/src/main/persistence/history-store';
import type { HistoryCreate } from '../../app/src/shared/schemas/history';
import { TRANSCRIPT_MAX_UTF8_BYTES } from '../../app/src/shared/schemas/transcription';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const Database = createRequire(new URL('../../app/package.json', import.meta.url))(
  'better-sqlite3',
) as typeof BetterSqlite3;

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

function historyInput(createdAt: number, rawText: string): HistoryCreate {
  return {
    createdAt,
    dictationMode: 'quick',
    processingMode: 'raw',
    outcome: 'raw-completed',
    rawText,
    processedText: null,
    providerId: null,
    modelId: null,
    fellBack: false,
    errorCategory: null,
    voiceTrigger: null,
    voiceSnippet: null,
    screenshotFilename: null,
  };
}

describe('HistoryStore', () => {
  it('migrates transactionally and supports create/read/delete with stable pagination', async () => {
    const directory = await createTestDirectory('history');
    directories.push(directory);
    const path = join(directory, 'history.db');
    const store = new HistoryStore(path);
    const first = store.create(historyInput(1, 'first'));
    const second = store.create(historyInput(2, 'second'));
    store.create(historyInput(3, 'third'));

    expect(store.getById(second.id)?.rawText).toBe('second');
    const pageOne = store.list(2);
    expect(pageOne.items.map((item) => item.rawText)).toEqual(['third', 'second']);
    expect(pageOne.nextCursor).not.toBeNull();
    const pageTwo = store.list(2, pageOne.nextCursor);
    expect(pageTwo.items.map((item) => item.rawText)).toEqual(['first']);
    expect(pageTwo.nextCursor).toBeNull();
    expect(store.deleteById(first.id)).toBe(true);
    expect(store.deleteById(first.id)).toBe(false);
    expect(store.deleteAll()).toBe(2);
    expect(store.list().items).toHaveLength(0);
    store.close();
  });

  it('migrates legacy multibyte transcripts to the bounded history contract', async () => {
    const directory = await createTestDirectory('history-legacy-transcript');
    directories.push(directory);
    const path = join(directory, 'history.db');
    const initial = new HistoryStore(path);
    const entry = initial.create(historyInput(1, 'initial'));
    initial.close();
    const legacy = new Database(path);
    legacy
      .prepare('UPDATE history SET raw_text = ? WHERE id = ?')
      .run('é'.repeat(600_000), entry.id);
    legacy.pragma('user_version = 1');
    legacy.close();

    const migrated = new HistoryStore(path);
    const rawText = migrated.getById(entry.id)?.rawText;
    expect(rawText).not.toBeNull();
    expect(Buffer.byteLength(rawText ?? '', 'utf8')).toBe(TRANSCRIPT_MAX_UTF8_BYTES);
    expect(migrated.list().items).toHaveLength(1);
    expect(migrated.deleteById(entry.id)).toBe(true);
    migrated.close();
  });

  it('updates only supplied fields, supports explicit null, and validates the full result', async () => {
    const directory = await createTestDirectory('history-update');
    directories.push(directory);
    const path = join(directory, 'history.db');
    const store = new HistoryStore(path);
    const created = store.create(historyInput(10, 'raw words'));

    const updated = store.update(created.id, {
      processingMode: 'smart',
      outcome: 'smart-completed',
      processedText: 'Clean words.',
      providerId: 'openai',
      modelId: 'model-1',
    });
    expect(updated).toMatchObject({
      id: created.id,
      createdAt: created.createdAt,
      rawText: 'raw words',
      processingMode: 'smart',
      processedText: 'Clean words.',
      providerId: 'openai',
    });

    const cleared = store.update(created.id, { providerId: null, processedText: null });
    expect(cleared?.providerId).toBeNull();
    expect(cleared?.processedText).toBeNull();
    expect(cleared?.rawText).toBe('raw words');
    store.close();

    const reopened = new HistoryStore(path);
    expect(reopened.getById(created.id)).toEqual(cleared);
    reopened.close();
  });

  it('returns null for a missing update and rejects empty, unknown, or invalid updates', async () => {
    const directory = await createTestDirectory('history-invalid-update');
    directories.push(directory);
    const store = new HistoryStore(join(directory, 'history.db'));
    const created = store.create(historyInput(1, 'words'));

    expect(store.update(randomUUID(), { rawText: 'missing' })).toBeNull();
    expect(() => store.update(created.id, {})).toThrow();
    expect(() => store.update(created.id, { unknown: true } as never)).toThrow();
    expect(() => store.update('not-a-uuid', { rawText: 'bad id' })).toThrow();
    expect(() => store.update(created.id, { screenshotFilename: '../bad.jpg' })).toThrow();
    store.close();
  });

  it('prunes only rows older than the retention cutoff', async () => {
    const directory = await createTestDirectory('history-prune');
    directories.push(directory);
    const store = new HistoryStore(join(directory, 'history.db'));
    store.create(historyInput(99, 'old'));
    store.create(historyInput(100, 'boundary'));
    store.create(historyInput(101, 'new'));

    expect(store.deleteOlderThan(100)).toBe(1);
    expect(store.list().items.map((item) => item.rawText)).toEqual(['new', 'boundary']);
    expect(() => store.deleteOlderThan(-1)).toThrow();
    store.close();
  });

  it('validates every record before insertion', async () => {
    const directory = await createTestDirectory('history-invalid-create');
    directories.push(directory);
    const store = new HistoryStore(join(directory, 'history.db'));
    expect(() =>
      store.create({ ...historyInput(1, 'bad'), screenshotFilename: '../bad.jpg' }),
    ).toThrow();
    store.close();
  });
});
