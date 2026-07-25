import {
  closeSync,
  constants,
  fstatSync,
  lstatSync,
  openSync,
  readFileSync,
  readdirSync,
  rmSync,
  type Dirent,
} from 'node:fs';
import { nativeImage } from 'electron';
import { join } from 'node:path';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { HistoryStore } from '../persistence/history-store';
import type { SettingsStore } from '../persistence/settings-store';
import {
  HistoryIdSchema,
  HistoryListRequestSchema,
  HistoryPageSchema,
  type HistoryCleanupStatus,
  type HistoryDeleteAllResult,
  type HistoryDeleteResult,
  type HistoryPage,
  type HistoryRetentionResult,
} from '../../shared/schemas/history';
import { mapSessionHistoryOutcome, type SessionHistoryRecord } from './session-history-mapper';
import {
  activePendingScreenshotNames,
  commitPendingScreenshot,
  thumbnailFilename,
} from '../screenshot/screenshot-retention';

const DAY_MS = 24 * 60 * 60 * 1_000;

export interface HistoryClipboard {
  writeText(text: string): void;
}

export interface HistoryScreenshotFiles {
  list(directory: string): Dirent[];
  remove(path: string): void;
  isRegularFile(path: string): boolean;
}

const nativeScreenshotFiles: HistoryScreenshotFiles = {
  list: (directory) => readdirSync(directory, { withFileTypes: true }),
  remove: (path) => rmSync(path, { force: true }),
  isRegularFile: (path) => lstatSync(path).isFile(),
};

export class HistoryService {
  readonly #store: HistoryStore;
  readonly #settings: SettingsStore;
  readonly #events: IpcEventEmitter;
  readonly #clipboard: HistoryClipboard;
  readonly #screenshotsDirectory: string;
  readonly #files: HistoryScreenshotFiles;
  readonly #now: () => number;
  readonly #thumbnailReadBoundary: ((path: string) => void) | undefined;
  #revision = 0;

  constructor(options: {
    readonly store: HistoryStore;
    readonly settings: SettingsStore;
    readonly events: IpcEventEmitter;
    readonly clipboard: HistoryClipboard;
    readonly screenshotsDirectory: string;
    readonly files?: HistoryScreenshotFiles;
    readonly now?: () => number;
    readonly thumbnailReadBoundary?: (path: string) => void;
  }) {
    this.#store = options.store;
    this.#settings = options.settings;
    this.#events = options.events;
    this.#clipboard = options.clipboard;
    this.#screenshotsDirectory = options.screenshotsDirectory;
    this.#files = options.files ?? nativeScreenshotFiles;
    this.#now = options.now ?? Date.now;
    this.#thumbnailReadBoundary = options.thumbnailReadBoundary;
  }

  pruneAtStartup(): HistoryRetentionResult {
    const deletedCount = this.#pruneExpired();
    return {
      deletedCount,
      screenshotCleanup: cleanupStatus(this.#scavengeOrphanedScreenshots()),
    };
  }

  async pruneAtStartupDeferred(
    batchSize = 64,
    signal?: AbortSignal,
  ): Promise<HistoryRetentionResult> {
    if (!Number.isSafeInteger(batchSize) || batchSize < 1)
      throw new Error('History cleanup batch size is invalid');
    const deletedCount = this.#pruneExpired();
    const cleanup = this.#scavengeCandidates();
    for (let index = 0; index < cleanup.entries.length; index += batchSize) {
      if (signal?.aborted === true) break;
      for (const entry of cleanup.entries.slice(index, index + batchSize)) {
        recordCleanup(cleanup.tally, this.#removeScreenshot(entry.name));
      }
      await new Promise<void>((resolveWait) => setImmediate(resolveWait));
    }
    return { deletedCount, screenshotCleanup: cleanupStatus(cleanup.tally) };
  }

  record(outcome: SessionHistoryRecord): boolean {
    if (!this.#settings.get().privacy.historyEnabled) return false;
    const mapped = mapSessionHistoryOutcome({ ...outcome, createdAt: this.#now() });
    const filename = mapped.screenshotFilename;
    if (filename === null) {
      this.#store.create(mapped);
    } else {
      // Promotion and row creation are one synchronous critical section. Promotion is first, so
      // interruption can leave an orphan for scavenging but can never create a dangling row.
      commitPendingScreenshot(this.#screenshotsDirectory, filename, (promoted) => {
        this.#store.create({ ...mapped, screenshotFilename: promoted });
      });
    }

    // The row is authoritative from this point. Retention cleanup and renderer notification are
    // ancillary and must not cause the controller to disown a successfully committed row.
    try {
      const before = this.#store.listScreenshotFilenames();
      this.#pruneExpired();
      this.#removeUnreferencedScreenshots(before);
    } catch {
      // Startup pruning/scavenging retries ancillary cleanup.
    }
    this.#changed();
    return true;
  }

  list(input: unknown): HistoryPage {
    const request = HistoryListRequestSchema.parse(input);
    const page = this.#store.list(request.limit ?? 25, request.cursor ?? null);
    return HistoryPageSchema.parse({
      items: page.items.map(({ screenshotFilename, ...record }) => ({
        ...record,
        hasScreenshot:
          screenshotFilename !== null && this.#isRetainedScreenshot(screenshotFilename),
      })),
      nextCursor: page.nextCursor,
      revision: this.#revision,
    });
  }

  deleteById(id: string): HistoryDeleteResult {
    const parsedId = HistoryIdSchema.parse(id);
    const filename = this.#store.getById(parsedId)?.screenshotFilename ?? null;
    const deleted = this.#store.deleteById(parsedId);
    const cleanup = deleted
      ? this.#removeUnreferencedScreenshots(filename === null ? [] : [filename])
      : EMPTY_CLEANUP;
    if (deleted) this.#changed();
    return {
      deleted,
      revision: this.#revision,
      screenshotCleanup: cleanupStatus(cleanup),
    };
  }

  deleteAll(): HistoryDeleteAllResult {
    const deletedCount = this.#store.deleteAll();
    const cleanup = this.#scavengeOrphanedScreenshots();
    if (deletedCount > 0) this.#changed();
    return {
      deletedCount,
      revision: this.#revision,
      screenshotCleanup: cleanupStatus(cleanup),
    };
  }

  thumbnail(id: string): { readonly base64: string } | null {
    const record = this.#store.getById(HistoryIdSchema.parse(id));
    if (record?.screenshotFilename === null || record?.screenshotFilename === undefined)
      return null;
    const contents = readVerifiedJpegThumbnail(
      join(this.#screenshotsDirectory, thumbnailFilename(record.screenshotFilename)),
      this.#thumbnailReadBoundary,
    );
    return contents === null ? null : { base64: contents.toString('base64') };
  }

  copy(id: string): { readonly copied: true } {
    const record = this.#store.getById(HistoryIdSchema.parse(id));
    if (record === null) throw new Error('History entry was not found');
    const text = record.processedText ?? record.voiceSnippet ?? record.rawText;
    if (text === null) throw new Error('History entry has no copyable text');
    this.#clipboard.writeText(text);
    return { copied: true };
  }

  #pruneExpired(): number {
    const days = this.#settings.get().privacy.historyRetentionDays;
    return days === null ? 0 : this.#store.deleteOlderThan(this.#now() - days * DAY_MS);
  }

  #removeUnreferencedScreenshots(candidates: readonly string[]): CleanupTally {
    const tally = mutableCleanup();
    if (candidates.length === 0) return tally;
    const retained = new Set(this.#store.listScreenshotFilenames());
    for (const filename of new Set(candidates)) {
      if (!retained.has(filename)) recordCleanup(tally, this.#removeScreenshot(filename));
    }
    return tally;
  }

  #scavengeOrphanedScreenshots(): CleanupTally {
    const cleanup = this.#scavengeCandidates();
    for (const entry of cleanup.entries) {
      recordCleanup(cleanup.tally, this.#removeScreenshot(entry.name));
    }
    return cleanup.tally;
  }

  #scavengeCandidates(): { readonly entries: Dirent[]; readonly tally: CleanupTally } {
    const tally = mutableCleanup();
    const filenames = this.#store.listScreenshotFilenames();
    const retained = new Set([
      ...filenames,
      ...filenames.map((filename) => thumbnailFilename(filename)),
      ...activePendingScreenshotNames(this.#screenshotsDirectory),
    ]);
    let entries: Dirent[];
    try {
      entries = this.#files.list(this.#screenshotsDirectory);
    } catch {
      tally.unattested = true;
      return { entries: [], tally };
    }
    return {
      entries: entries.filter(
        (entry) => (entry.isFile() || entry.isSymbolicLink()) && !retained.has(entry.name),
      ),
      tally,
    };
  }

  #removeScreenshot(filename: string): boolean {
    try {
      this.#files.remove(join(this.#screenshotsDirectory, filename));
      this.#files.remove(join(this.#screenshotsDirectory, thumbnailFilename(filename)));
      return true;
    } catch {
      // The database deletion is authoritative. File cleanup is retried by app-owned screenshot
      // scavenging at the next startup or Delete All operation.
      return false;
    }
  }

  #isRetainedScreenshot(filename: string): boolean {
    try {
      return this.#files.isRegularFile(join(this.#screenshotsDirectory, filename));
    } catch {
      return false;
    }
  }

  #changed(): void {
    this.#revision += 1;
    try {
      this.#events.send('history:changed', { revision: this.#revision });
    } catch {
      // A committed history mutation remains authoritative if renderer notification fails.
    }
  }
}

interface CleanupTally {
  attempted: number;
  succeeded: number;
  unattested: boolean;
}

const EMPTY_CLEANUP: CleanupTally = Object.freeze({
  attempted: 0,
  succeeded: 0,
  unattested: false,
});

function mutableCleanup(): CleanupTally {
  return { attempted: 0, succeeded: 0, unattested: false };
}

function recordCleanup(tally: CleanupTally, succeeded: boolean): void {
  tally.attempted += 1;
  if (succeeded) tally.succeeded += 1;
}

function cleanupStatus(tally: CleanupTally): HistoryCleanupStatus {
  const failed = tally.attempted - tally.succeeded;
  if (!tally.unattested && failed === 0) return 'complete';
  return tally.succeeded > 0 ? 'partial' : 'pending';
}

function readVerifiedJpegThumbnail(
  path: string,
  afterLstat?: (path: string) => void,
): Buffer | null {
  let descriptor: number | null = null;
  try {
    const before = lstatSync(path);
    if (
      !before.isFile() ||
      before.isSymbolicLink() ||
      before.size < 4 ||
      before.size > 360 * 1_024
    ) {
      return null;
    }
    afterLstat?.(path);
    descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const opened = fstatSync(descriptor);
    if (
      !opened.isFile() ||
      opened.size !== before.size ||
      opened.dev !== before.dev ||
      opened.ino !== before.ino
    ) {
      return null;
    }
    const encoded = readFileSync(descriptor);
    if (
      encoded.length < 4 ||
      encoded[0] !== 0xff ||
      encoded[1] !== 0xd8 ||
      encoded[2] !== 0xff ||
      encoded.at(-2) !== 0xff ||
      encoded.at(-1) !== 0xd9
    ) {
      return null;
    }
    const decoded = nativeImage.createFromBuffer(encoded);
    if (decoded.isEmpty()) return null;
    const size = decoded.getSize();
    if (size.width < 1 || size.height < 1 || size.width > 4_096 || size.height > 4_096) return null;
    const safe = decoded.toJPEG(80);
    return safe.length >= 4 && safe.length <= 360 * 1_024 ? safe : null;
  } catch {
    return null;
  } finally {
    if (descriptor !== null) closeSync(descriptor);
  }
}
