import { randomUUID } from 'node:crypto';
import { chmodSync, renameSync, rmSync, writeFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { nativeImage } from 'electron';
import type { CapturedScreenshot } from './screenshot-service';

const active = new Map<string, RetainedScreenshot>();

type RetentionState = 'pending' | 'promoting' | 'row-committed' | 'disposed';

export class RetainedScreenshot {
  readonly filename: string;
  readonly #directory: string;
  readonly #pendingFilename: string;
  readonly #pendingThumbnailFilename: string;
  #state: RetentionState = 'pending';

  constructor(directory: string, screenshot: CapturedScreenshot) {
    this.#directory = resolve(directory);
    this.filename = `${randomUUID()}.jpg`;
    this.#pendingFilename = `.pending-${this.filename}`;
    this.#pendingThumbnailFilename = `.pending-${thumbnailFilename(this.filename)}`;
    const full = Buffer.from(screenshot.image.base64, 'base64');
    const source = nativeImage.createFromBuffer(full);
    if (source.isEmpty()) throw new Error('Screenshot could not be decoded');
    const thumbnail = source.resize({ width: 320, quality: 'good' }).toJPEG(80);
    if (thumbnail.length === 0) throw new Error('Screenshot thumbnail could not be encoded');
    this.#writeAtomic(this.#pendingFilename, full);
    try {
      this.#writeAtomic(this.#pendingThumbnailFilename, thumbnail);
      active.set(registryKey(this.#directory, this.filename), this);
    } catch (error: unknown) {
      this.#removeAll();
      throw error;
    }
  }

  commit(): void {
    // HistoryService owns the authoritative transition. This acknowledges controller ownership
    // only after the row is known to exist; it must never promote files independently.
    if (this.#state === 'row-committed') active.delete(registryKey(this.#directory, this.filename));
  }

  cleanup(): void {
    if (this.#state === 'row-committed' || this.#state === 'disposed') return;
    if (this.#state === 'promoting') return;
    this.#state = 'disposed';
    active.delete(registryKey(this.#directory, this.filename));
    this.#removeAll();
  }

  promoteAndCommit(commitRow: (filename: string) => void): void {
    if (this.#state !== 'pending') throw new Error('Screenshot is not pending promotion');
    this.#state = 'promoting';
    try {
      renameSync(
        join(this.#directory, this.#pendingFilename),
        join(this.#directory, this.filename),
      );
      renameSync(
        join(this.#directory, this.#pendingThumbnailFilename),
        join(this.#directory, thumbnailFilename(this.filename)),
      );
      // File promotion intentionally precedes the database transaction. Process interruption can
      // therefore leave only an orphan, which startup scavenging can remove, never a dangling row.
      commitRow(this.filename);
      this.#state = 'row-committed';
      active.delete(registryKey(this.#directory, this.filename));
    } catch (error: unknown) {
      this.#state = 'disposed';
      active.delete(registryKey(this.#directory, this.filename));
      this.#removeAll();
      throw error;
    }
  }

  pendingNames(directory: string): readonly string[] {
    return resolve(directory) === this.#directory
      ? [this.#pendingFilename, this.#pendingThumbnailFilename]
      : [];
  }

  #removeAll(): void {
    for (const filename of [
      this.#pendingFilename,
      this.#pendingThumbnailFilename,
      this.filename,
      thumbnailFilename(this.filename),
    ]) {
      rmSync(join(this.#directory, filename), { force: true });
    }
  }

  #writeAtomic(filename: string, contents: Buffer): void {
    const destination = join(this.#directory, filename);
    const temporary = `${destination}.${randomUUID()}.tmp`;
    try {
      writeFileSync(temporary, contents, { flag: 'wx', mode: 0o600 });
      chmodSync(temporary, 0o600);
      renameSync(temporary, destination);
    } finally {
      rmSync(temporary, { force: true });
    }
  }
}

export function commitPendingScreenshot(
  directory: string,
  filename: string,
  commitRow: (filename: string | null) => void,
): boolean {
  const normalizedDirectory = resolve(directory);
  const retained = active.get(registryKey(normalizedDirectory, filename));
  if (retained === undefined) {
    // Missing/revoked pending ownership is never written into a new row.
    commitRow(null);
    return false;
  }
  retained.promoteAndCommit((promoted) => commitRow(promoted));
  return true;
}

export function activePendingScreenshotNames(directory: string): ReadonlySet<string> {
  const normalizedDirectory = resolve(directory);
  const names = new Set<string>();
  for (const retained of active.values()) {
    for (const name of retained.pendingNames(normalizedDirectory)) names.add(name);
  }
  return names;
}

export function thumbnailFilename(filename: string): string {
  return filename.replace(/\.jpg$/i, '.thumb.jpg');
}

function registryKey(directory: string, filename: string): string {
  return `${resolve(directory)}\0${filename}`;
}
