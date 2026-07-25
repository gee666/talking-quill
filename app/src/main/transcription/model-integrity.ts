import { createHash } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import { lstat, open, type FileHandle } from 'node:fs/promises';
import type { VerifiedModelFileIdentity } from '../../shared/schemas/model-manifest';

export interface FileIntegrity {
  readonly exists: boolean;
  readonly valid: boolean;
  readonly size: number;
  readonly identity: Omit<VerifiedModelFileIdentity, 'path'> | null;
}

export async function inspectFile(
  path: string,
  expectedSize: number,
  expectedSha256: string,
  hash: boolean,
  signal?: AbortSignal,
): Promise<FileIntegrity> {
  const before = await safeLstat(path);
  if (before === null) return { exists: false, valid: false, size: 0, identity: null };
  if (!before.isFile() || before.isSymbolicLink()) {
    return { exists: true, valid: false, size: before.size, identity: null };
  }
  if (before.size !== expectedSize) {
    return { exists: true, valid: false, size: before.size, identity: fileIdentity(before) };
  }
  if (!hash) {
    return { exists: true, valid: true, size: before.size, identity: fileIdentity(before) };
  }

  signal?.throwIfAborted();
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat();
    if (!opened.isFile() || !sameFileIdentity(before, opened)) {
      return { exists: true, valid: false, size: opened.size, identity: null };
    }
    const sha256 = await hashHandle(handle, signal);
    const [afterHandle, afterPath] = await Promise.all([handle.stat(), safeLstat(path)]);
    if (
      afterPath === null ||
      afterPath.isSymbolicLink() ||
      !afterPath.isFile() ||
      !sameFileIdentity(opened, afterHandle) ||
      !sameFileIdentity(afterHandle, afterPath)
    ) {
      return { exists: true, valid: false, size: afterHandle.size, identity: null };
    }
    return {
      exists: true,
      valid: sha256 === expectedSha256,
      size: afterHandle.size,
      identity: fileIdentity(afterHandle),
    };
  } finally {
    await handle.close();
  }
}

export async function sha256File(path: string, signal?: AbortSignal): Promise<string> {
  signal?.throwIfAborted();
  const before = await lstat(path);
  if (!before.isFile() || before.isSymbolicLink()) throw new Error('File is not a regular file');
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat();
    if (!opened.isFile() || !sameFileIdentity(before, opened)) {
      throw new Error('File identity changed during secure open');
    }
    const digest = await hashHandle(handle, signal);
    const [afterHandle, afterPath] = await Promise.all([handle.stat(), lstat(path)]);
    if (!sameFileIdentity(opened, afterHandle) || !sameFileIdentity(afterHandle, afterPath)) {
      throw new Error('File identity changed during checksum verification');
    }
    return digest;
  } finally {
    await handle.close();
  }
}

export function sameVerifiedIdentity(
  expected: Omit<VerifiedModelFileIdentity, 'path'>,
  actual: Stats,
): boolean {
  return (
    expected.size === actual.size &&
    expected.mtimeMs === actual.mtimeMs &&
    expected.ctimeMs === actual.ctimeMs &&
    expected.birthtimeMs === actual.birthtimeMs &&
    expected.device === String(actual.dev) &&
    expected.inode === String(actual.ino)
  );
}

async function hashHandle(handle: FileHandle, signal?: AbortSignal): Promise<string> {
  const digest = createHash('sha256');
  const stream = handle.createReadStream({ autoClose: false, start: 0, signal });
  for await (const chunk of stream) {
    signal?.throwIfAborted();
    const value: unknown = chunk;
    if (!Buffer.isBuffer(value)) throw new Error('File stream returned a non-buffer chunk');
    digest.update(value);
  }
  return digest.digest('hex');
}

async function safeLstat(path: string): Promise<Stats | null> {
  try {
    return await lstat(path);
  } catch (error: unknown) {
    if (isMissing(error)) return null;
    throw error;
  }
}

function fileIdentity(metadata: Stats): Omit<VerifiedModelFileIdentity, 'path'> {
  return {
    size: metadata.size,
    mtimeMs: metadata.mtimeMs,
    ctimeMs: metadata.ctimeMs,
    birthtimeMs: metadata.birthtimeMs,
    device: String(metadata.dev),
    inode: String(metadata.ino),
  };
}

function sameFileIdentity(first: Stats, second: Stats): boolean {
  return sameVerifiedIdentity(fileIdentity(first), second);
}

function isMissing(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { readonly code?: unknown }).code === 'ENOENT'
  );
}
