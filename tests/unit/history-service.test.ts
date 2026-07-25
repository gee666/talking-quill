import {
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  rmSync,
  symlinkSync,
  writeFileSync,
} from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  HistoryService,
  type HistoryScreenshotFiles,
} from '../../app/src/main/history/history-service';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import { HistoryStore } from '../../app/src/main/persistence/history-store';
import {
  RetainedScreenshot,
  thumbnailFilename,
} from '../../app/src/main/screenshot/screenshot-retention';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

vi.mock('electron', () => ({
  nativeImage: {
    createFromBuffer: (contents: Buffer) => ({
      isEmpty: () =>
        contents.length < 4 ||
        contents[0] !== 0xff ||
        contents[1] !== 0xd8 ||
        contents.at(-2) !== 0xff ||
        contents.at(-1) !== 0xd9,
      getSize: () => ({ width: 64, height: 64 }),
      resize: () => ({ toJPEG: () => Buffer.from(contents) }),
      toJPEG: () => Buffer.from(contents),
    }),
  },
}));

const VALID_JPEG = Buffer.from(
  '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCABAAEADASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAYI/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AnQCOaRAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAf//Z',
  'base64',
);

const directories: string[] = [];
afterEach(async () => Promise.all(directories.splice(0).map(removeTestDirectory)));

async function fixture(
  options: {
    enabled?: boolean;
    retention?: 7 | 30 | 90 | null;
    files?: HistoryScreenshotFiles;
    thumbnailReadBoundary?: (path: string) => void;
  } = {},
) {
  const directory = await createTestDirectory('history-service');
  directories.push(directory);
  const store = new HistoryStore(join(directory, 'history.db'));
  const screenshotsDirectory = join(directory, 'screenshots');
  mkdirSync(screenshotsDirectory);
  const settings = {
    get: () => ({
      ...structuredClone(DEFAULT_SETTINGS),
      privacy: {
        historyEnabled: options.enabled ?? true,
        historyRetentionDays: options.retention ?? null,
      },
    }),
  } as SettingsStore;
  const send = vi.fn();
  const writeText = vi.fn();
  const service = new HistoryService({
    store,
    settings,
    events: { send } as unknown as IpcEventEmitter,
    clipboard: { writeText },
    screenshotsDirectory,
    ...(options.files === undefined ? {} : { files: options.files }),
    ...(options.thumbnailReadBoundary === undefined
      ? {}
      : { thumbnailReadBoundary: options.thumbnailReadBoundary }),
    now: () => 100 * 24 * 60 * 60 * 1_000,
  });
  return { service, store, send, writeText, screenshotsDirectory };
}

describe('HistoryService', () => {
  it('records, lists without filenames, copies by id, and publishes revisions', async () => {
    const test = await fixture();
    expect(
      test.service.record({
        kind: 'raw-completed',
        dictationMode: 'quick',
        processingMode: 'raw',
        rawText: 'private transcript',
      }),
    ).toBe(true);
    const page = test.service.list({ limit: 10 });
    expect(page).toMatchObject({ revision: 1, nextCursor: null });
    const first = page.items[0];
    expect(first).toMatchObject({ rawText: 'private transcript', hasScreenshot: false });
    expect(first).not.toHaveProperty('screenshotFilename');
    if (first === undefined) throw new Error('Expected a history item');
    test.service.copy(first.id);
    expect(test.writeText).toHaveBeenCalledWith('private transcript');
    expect(test.send).toHaveBeenCalledWith('history:changed', { revision: 1 });
    test.store.close();
  });

  it('suppresses writes when history is disabled', async () => {
    const test = await fixture({ enabled: false });
    expect(
      test.service.record({
        kind: 'raw-completed',
        dictationMode: 'quick',
        processingMode: 'raw',
        rawText: 'not stored',
      }),
    ).toBe(false);
    expect(test.store.list().items).toEqual([]);
    expect(test.send).not.toHaveBeenCalled();
    test.store.close();
  });

  it('prunes at startup against an injected clock and preserves the cutoff boundary', async () => {
    const test = await fixture({ retention: 7 });
    const now = 100 * 24 * 60 * 60 * 1_000;
    const cutoff = now - 7 * 24 * 60 * 60 * 1_000;
    const input = (createdAt: number, rawText: string) => ({
      createdAt,
      dictationMode: 'quick' as const,
      processingMode: 'raw' as const,
      outcome: 'raw-completed' as const,
      rawText,
      processedText: null,
      providerId: null,
      modelId: null,
      fellBack: false,
      errorCategory: null,
      voiceTrigger: null,
      voiceSnippet: null,
      screenshotFilename: null,
    });
    test.store.create(input(cutoff - 1, 'old'));
    test.store.create(input(cutoff, 'boundary'));
    test.store.create(input(now, 'new'));
    expect(test.service.pruneAtStartup()).toEqual({
      deletedCount: 1,
      screenshotCleanup: 'complete',
    });
    expect(test.store.list().items.map((item) => item.rawText)).toEqual(['new', 'boundary']);
    test.store.close();
  });

  it('defers startup screenshot cleanup in bounded batches', async () => {
    const test = await fixture({ retention: 7 });
    for (let index = 0; index < 130; index += 1) {
      writeFileSync(join(test.screenshotsDirectory, `orphan-${String(index)}.jpg`), 'orphan');
    }
    await expect(test.service.pruneAtStartupDeferred(16)).resolves.toEqual({
      deletedCount: 0,
      screenshotCleanup: 'complete',
    });
    expect(
      readdirSync(test.screenshotsDirectory).filter((name) => name.startsWith('orphan-')),
    ).toEqual([]);
    test.store.close();
  });

  it('yields before startup enumeration and reports aborted physical cleanup as pending', async () => {
    const list = vi.fn((directory: string) => readdirSync(directory, { withFileTypes: true }));
    const files: HistoryScreenshotFiles = {
      list,
      remove: (path) => rmSync(path, { force: true }),
      isRegularFile: (path) => lstatSync(path).isFile(),
    };
    const test = await fixture({ files });
    writeFileSync(join(test.screenshotsDirectory, 'orphan.jpg'), 'orphan');

    const pending = test.service.pruneAtStartupDeferred(1);
    expect(list).not.toHaveBeenCalled();
    await expect(pending).resolves.toMatchObject({ screenshotCleanup: 'complete' });
    expect(list).toHaveBeenCalledOnce();

    writeFileSync(join(test.screenshotsDirectory, 'cancelled-orphan.jpg'), 'orphan');
    const controller = new AbortController();
    controller.abort();
    await expect(test.service.pruneAtStartupDeferred(1, controller.signal)).resolves.toEqual({
      deletedCount: 0,
      screenshotCleanup: 'pending',
    });
    expect(existsSync(join(test.screenshotsDirectory, 'cancelled-orphan.jpg'))).toBe(true);
    test.store.close();
  });

  it('skips retention queries and screenshot scans for unlimited history records', async () => {
    const test = await fixture({ retention: null });
    const prune = vi.spyOn(test.store, 'deleteOlderThanWithScreenshots');
    const retained = vi.spyOn(test.store, 'listRetainedScreenshotFilenames');
    const all = vi.spyOn(test.store, 'listScreenshotFilenames');

    expect(
      test.service.record({
        kind: 'raw-completed',
        dictationMode: 'quick',
        processingMode: 'raw',
        rawText: 'retained without cleanup scans',
      }),
    ).toBe(true);

    expect(prune).not.toHaveBeenCalled();
    expect(retained).not.toHaveBeenCalled();
    expect(all).not.toHaveBeenCalled();
    test.store.close();
  });

  it('removes screenshot files on individual, bulk, and retention deletion', async () => {
    const test = await fixture({ retention: 7 });
    const day = 24 * 60 * 60 * 1_000;
    const createScreenshotEntry = (createdAt: number, rawText: string, filename: string) => {
      writeFileSync(join(test.screenshotsDirectory, filename), rawText);
      return test.store.create({
        createdAt,
        dictationMode: 'quick',
        processingMode: 'smart',
        outcome: 'smart-completed',
        rawText,
        processedText: rawText,
        providerId: 'ollama',
        modelId: 'model',
        fellBack: false,
        errorCategory: null,
        voiceTrigger: null,
        voiceSnippet: null,
        screenshotFilename: filename,
      });
    };

    const individual = createScreenshotEntry(99 * day, 'individual', 'individual.jpg');
    expect(test.service.deleteById(individual.id).deleted).toBe(true);
    expect(existsSync(join(test.screenshotsDirectory, 'individual.jpg'))).toBe(false);

    createScreenshotEntry(90 * day, 'expired', 'expired.jpg');
    createScreenshotEntry(99 * day, 'retained', 'retained.jpg');
    writeFileSync(join(test.screenshotsDirectory, 'orphan.jpg'), 'orphan');
    expect(test.service.pruneAtStartup()).toEqual({
      deletedCount: 1,
      screenshotCleanup: 'complete',
    });
    expect(existsSync(join(test.screenshotsDirectory, 'expired.jpg'))).toBe(false);
    expect(existsSync(join(test.screenshotsDirectory, 'orphan.jpg'))).toBe(false);
    expect(existsSync(join(test.screenshotsDirectory, 'retained.jpg'))).toBe(true);

    expect(test.service.deleteAll().deletedCount).toBe(1);
    expect(existsSync(join(test.screenshotsDirectory, 'retained.jpg'))).toBe(false);
    test.store.close();
  });

  it('applies retention on record and scavenges app-owned screenshots even with no rows', async () => {
    const test = await fixture({ retention: 7 });
    const day = 24 * 60 * 60 * 1_000;
    test.store.create({
      createdAt: 90 * day,
      dictationMode: 'quick',
      processingMode: 'raw',
      outcome: 'raw-completed',
      rawText: 'expired',
      processedText: null,
      providerId: null,
      modelId: null,
      fellBack: false,
      errorCategory: null,
      voiceTrigger: null,
      voiceSnippet: null,
      screenshotFilename: null,
    });
    test.service.record({
      kind: 'raw-completed',
      dictationMode: 'quick',
      processingMode: 'raw',
      rawText: 'retained',
    });
    expect(test.store.list().items.map((entry) => entry.rawText)).toEqual(['retained']);

    test.service.deleteAll();
    writeFileSync(join(test.screenshotsDirectory, 'orphan-without-row.jpg'), 'orphan');
    expect(test.service.deleteAll()).toMatchObject({ deletedCount: 0 });
    expect(existsSync(join(test.screenshotsDirectory, 'orphan-without-row.jpg'))).toBe(false);
    test.store.close();
  });

  it('reports individual screenshot cleanup failure and retries it at startup', async () => {
    let fail = true;
    const test = await fixture({
      files: screenshotFiles((path) => {
        if (fail && path.endsWith('retry.jpg')) throw new Error('locked');
        rmSync(path, { force: true });
      }),
    });
    const entry = createScreenshotHistory(test, 'retry.jpg', 99);
    expect(test.service.deleteById(entry.id)).toMatchObject({
      deleted: true,
      screenshotCleanup: 'pending',
    });
    expect(existsSync(join(test.screenshotsDirectory, 'retry.jpg'))).toBe(true);
    fail = false;
    expect(test.service.pruneAtStartup()).toMatchObject({ screenshotCleanup: 'complete' });
    expect(existsSync(join(test.screenshotsDirectory, 'retry.jpg'))).toBe(false);
    test.store.close();
  });

  it('reports partial Delete All cleanup without exposing paths', async () => {
    const test = await fixture({
      files: screenshotFiles((path) => {
        if (path.endsWith('locked.jpg')) throw new Error(`private path: ${path}`);
        rmSync(path, { force: true });
      }),
    });
    createScreenshotHistory(test, 'removed.jpg', 99);
    createScreenshotHistory(test, 'locked.jpg', 99);
    expect(test.service.deleteAll()).toEqual({
      deletedCount: 2,
      revision: 1,
      screenshotCleanup: 'partial',
    });
    expect(existsSync(join(test.screenshotsDirectory, 'removed.jpg'))).toBe(false);
    expect(existsSync(join(test.screenshotsDirectory, 'locked.jpg'))).toBe(true);
    test.store.close();
  });

  it('reports retention cleanup failure and retries orphan scavenging', async () => {
    let fail = true;
    const test = await fixture({
      retention: 7,
      files: screenshotFiles((path) => {
        if (fail) throw new Error('locked');
        rmSync(path, { force: true });
      }),
    });
    createScreenshotHistory(test, 'expired-locked.jpg', 90);
    expect(test.service.pruneAtStartup()).toEqual({
      deletedCount: 1,
      screenshotCleanup: 'pending',
    });
    fail = false;
    expect(test.service.pruneAtStartup()).toEqual({
      deletedCount: 0,
      screenshotCleanup: 'complete',
    });
    expect(existsSync(join(test.screenshotsDirectory, 'expired-locked.jpg'))).toBe(false);
    test.store.close();
  });

  it('promotes registered pending files before the row and never scavenges active pending files', async () => {
    const test = await fixture();
    const retained = createPendingScreenshot(test.screenshotsDirectory);
    const pendingBefore = readdirSync(test.screenshotsDirectory).filter((name) =>
      name.startsWith('.pending-'),
    );
    expect(pendingBefore).toHaveLength(2);
    expect(test.service.deleteAll()).toMatchObject({ deletedCount: 0 });
    expect(
      readdirSync(test.screenshotsDirectory).filter((name) => name.startsWith('.pending-')),
    ).toHaveLength(2);

    expect(
      test.service.record({
        kind: 'smart-completed',
        dictationMode: 'quick',
        processingMode: 'smart',
        rawText: 'raw',
        processedText: 'clean',
        providerId: 'ollama',
        modelId: 'vision',
        screenshotFilename: retained.filename,
      }),
    ).toBe(true);
    retained.commit();
    const row = test.store.list().items[0];
    expect(row?.screenshotFilename).toBe(retained.filename);
    expect(existsSync(join(test.screenshotsDirectory, retained.filename))).toBe(true);
    expect(existsSync(join(test.screenshotsDirectory, thumbnailFilename(retained.filename)))).toBe(
      true,
    );
    expect(
      readdirSync(test.screenshotsDirectory).some((name) => name.startsWith('.pending-')),
    ).toBe(false);
    test.store.close();
  });

  it('leaves no dangling row when promotion or row creation is interrupted', async () => {
    const test = await fixture();
    const retained = createPendingScreenshot(test.screenshotsDirectory);
    const create = vi.spyOn(test.store, 'create').mockImplementationOnce(() => {
      retained.cleanup();
      throw new Error('database interruption');
    });
    expect(() =>
      test.service.record({
        kind: 'smart-completed',
        dictationMode: 'quick',
        processingMode: 'smart',
        rawText: 'raw',
        processedText: 'clean',
        providerId: 'ollama',
        modelId: 'vision',
        screenshotFilename: retained.filename,
      }),
    ).toThrow('database interruption');
    expect(test.store.list().items).toEqual([]);
    expect(readdirSync(test.screenshotsDirectory)).toEqual([]);
    create.mockRestore();
    test.store.close();
  });

  it('keeps the authoritative row when ancillary cleanup or event publication fails', async () => {
    const test = await fixture({ retention: 7 });
    test.send.mockImplementation(() => {
      throw new Error('renderer gone');
    });
    vi.spyOn(test.store, 'deleteOlderThanWithScreenshots').mockImplementationOnce(() => {
      throw new Error('cleanup unavailable');
    });
    expect(
      test.service.record({
        kind: 'raw-completed',
        dictationMode: 'quick',
        processingMode: 'raw',
        rawText: 'authoritative',
      }),
    ).toBe(true);
    expect(test.store.list().items).toHaveLength(1);
    test.store.close();
  });

  it.each([
    ['malformed', Buffer.from('not an image')],
    ['truncated JPEG', VALID_JPEG.subarray(0, VALID_JPEG.length - 2)],
    ['non-JPEG', Buffer.from('89504e470d0a1a0a', 'hex')],
  ])('rejects %s thumbnails', async (_name, contents) => {
    const test = await fixture();
    const entry = createScreenshotHistory(test, 'unsafe.jpg', 99);
    writeFileSync(join(test.screenshotsDirectory, 'unsafe.thumb.jpg'), contents);
    expect(test.service.thumbnail(entry.id)).toBeNull();
    test.store.close();
  });

  it('decodes and re-encodes a valid JPEG but rejects symlinks and deletion races', async () => {
    const test = await fixture();
    const entry = createScreenshotHistory(test, 'safe.jpg', 99);
    const thumbnail = join(test.screenshotsDirectory, 'safe.thumb.jpg');
    writeFileSync(thumbnail, VALID_JPEG);
    const result = test.service.thumbnail(entry.id);
    expect(result).not.toBeNull();
    const safe = Buffer.from(result?.base64 ?? '', 'base64');
    expect([...safe.subarray(0, 3)]).toEqual([0xff, 0xd8, 0xff]);
    expect([...safe.subarray(-2)]).toEqual([0xff, 0xd9]);

    rmSync(thumbnail, { force: true });
    const target = join(test.screenshotsDirectory, 'target.jpg');
    writeFileSync(target, VALID_JPEG);
    try {
      symlinkSync(target, thumbnail);
      expect(test.service.thumbnail(entry.id)).toBeNull();
    } catch {
      // Symlink creation may require elevated Windows policy; secure-open behavior is exercised on
      // platforms where the test process can create one.
    }
    test.store.close();

    let removed = false;
    const raced = await fixture({
      thumbnailReadBoundary: (path) => {
        removed = true;
        rmSync(path, { force: true });
      },
    });
    const racedEntry = createScreenshotHistory(raced, 'race.jpg', 99);
    writeFileSync(join(raced.screenshotsDirectory, 'race.thumb.jpg'), VALID_JPEG);
    expect(raced.service.thumbnail(racedEntry.id)).toBeNull();
    expect(removed).toBe(true);
    raced.store.close();
  });

  it('deletes one and all without exposing an arbitrary clipboard write', async () => {
    const test = await fixture();
    for (const text of ['one', 'two']) {
      test.service.record({
        kind: 'raw-completed',
        dictationMode: 'quick',
        processingMode: 'raw',
        rawText: text,
      });
    }
    const first = test.service.list({}).items[0];
    if (first === undefined) throw new Error('Expected a history item');
    expect(test.service.deleteById(first.id)).toMatchObject({ deleted: true, revision: 3 });
    expect(test.service.deleteAll()).toMatchObject({ deletedCount: 1, revision: 4 });
    expect(() => test.service.copy(first.id)).toThrow('not found');
    test.store.close();
  });
});

function createPendingScreenshot(directory: string): RetainedScreenshot {
  return new RetainedScreenshot(directory, {
    image: { mimeType: 'image/jpeg', base64: VALID_JPEG.toString('base64') },
    width: 64,
    height: 64,
  });
}

function screenshotFiles(remove: (path: string) => void): HistoryScreenshotFiles {
  return {
    list: (directory) => readdirSync(directory, { withFileTypes: true }),
    remove,
    isRegularFile: (path) => lstatSync(path).isFile(),
  };
}

function createScreenshotHistory(
  test: Awaited<ReturnType<typeof fixture>>,
  filename: string,
  createdAtDays: number,
) {
  writeFileSync(join(test.screenshotsDirectory, filename), 'private screenshot');
  return test.store.create({
    createdAt: createdAtDays * 24 * 60 * 60 * 1_000,
    dictationMode: 'quick',
    processingMode: 'smart',
    outcome: 'smart-completed',
    rawText: 'raw',
    processedText: 'processed',
    providerId: 'ollama',
    modelId: 'model',
    fellBack: false,
    errorCategory: null,
    voiceTrigger: null,
    voiceSnippet: null,
    screenshotFilename: filename,
  });
}
