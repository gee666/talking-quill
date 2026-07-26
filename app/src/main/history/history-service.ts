import { lstatSync, readdirSync, rmSync, type Dirent } from 'node:fs';
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
import { readVerifiedJpegThumbnail } from './thumbnail-reader';
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
    const retention = this.#pruneExpired();
    return {
      deletedCount: retention.deletedCount,
      screenshotCleanup: cleanupStatus(this.#scavengeOrphanedScreenshots()),
    };
  }

  async pruneAtStartupDeferred(
    batchSize = 64,
    signal?: AbortSignal,
  ): Promise<HistoryRetentionResult> {
    if (!Number.isSafeInteger(batchSize) || batchSize < 1)
      throw new Error('History cleanup batch size is invalid');
    const retention = this.#pruneExpired();
    // Logical retention is authoritative, but filesystem enumeration is ancillary. Always yield
    // once before potentially walking a large screenshot directory after the renderer appears.
    await new Promise<void>((resolveWait) => setImmediate(resolveWait));
    if (isAborted(signal)) {
      return { deletedCount: retention.deletedCount, screenshotCleanup: 'pending' };
    }
    const cleanup = this.#scavengeCandidates();
    for (let index = 0; index < cleanup.entries.length; index += batchSize) {
      if (isAborted(signal)) {
        cleanup.tally.unattested = true;
        break;
      }
      for (const entry of cleanup.entries.slice(index, index + batchSize)) {
        recordCleanup(cleanup.tally, this.#removeScavengedEntry(entry.name));
      }
      await new Promise<void>((resolveWait) => setImmediate(resolveWait));
    }
    return {
      deletedCount: retention.deletedCount,
      screenshotCleanup: cleanupStatus(cleanup.tally),
    };
  }

  record(outcome: SessionHistoryRecord): boolean {
    const privacy = this.#settings.get().privacy;
    if (!privacy.historyEnabled) return false;
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
      const retention = this.#pruneExpired(privacy.historyRetentionDays);
      this.#removeUnreferencedScreenshots(retention.screenshotFilenames);
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

  #pruneExpired(retentionDays = this.#settings.get().privacy.historyRetentionDays): {
    readonly deletedCount: number;
    readonly screenshotFilenames: readonly string[];
  } {
    return retentionDays === null
      ? { deletedCount: 0, screenshotFilenames: [] }
      : this.#store.deleteOlderThanWithScreenshots(this.#now() - retentionDays * DAY_MS);
  }

  #removeUnreferencedScreenshots(candidates: readonly string[]): CleanupTally {
    const tally = mutableCleanup();
    if (candidates.length === 0) return tally;
    const unique = [...new Set(candidates)];
    const retained = this.#store.listRetainedScreenshotFilenames(unique);
    for (const filename of unique) {
      if (!retained.has(filename)) recordCleanup(tally, this.#removeScreenshot(filename));
    }
    return tally;
  }

  #scavengeOrphanedScreenshots(): CleanupTally {
    const cleanup = this.#scavengeCandidates();
    for (const entry of cleanup.entries) {
      recordCleanup(cleanup.tally, this.#removeScavengedEntry(entry.name));
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
    let removed = true;
    for (const candidate of [filename, thumbnailFilename(filename)]) {
      try {
        this.#files.remove(join(this.#screenshotsDirectory, candidate));
      } catch {
        // Original and thumbnail are independent privacy artifacts. Keep attempting both, then
        // report the pair as incomplete so startup scavenging retries any retained entry.
        removed = false;
      }
    }
    return removed;
  }

  #removeScavengedEntry(filename: string): boolean {
    try {
      // Every orphan is already an enumerated directory entry. Removing only that entry avoids
      // duplicate work and synthetic "*.thumb.thumb.jpg" probes for thumbnail entries.
      this.#files.remove(join(this.#screenshotsDirectory, filename));
      return true;
    } catch {
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

function isAborted(signal: AbortSignal | undefined): boolean {
  return signal?.aborted === true;
}
