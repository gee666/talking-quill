import { constants, type Stats } from 'node:fs';
import {
  MODEL_DOWNLOAD_HEADROOM_RATIO,
  MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
} from '../../shared/constants/whisper';
import { lstat, mkdir, open, readdir, rm, statfs, type FileHandle } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import type { VerifiedModelFileIdentity } from '../../shared/schemas/model-manifest';
import { ModelManagerError } from './errors';
import { sameVerifiedIdentity } from './model-integrity';

export interface ModelPartialWriter {
  write(bytes: Uint8Array): Promise<void>;
  sync(): Promise<void>;
  close(): Promise<void>;
}

export class FileHandlePartialWriter implements ModelPartialWriter {
  readonly #handle: FileHandle;

  constructor(handle: FileHandle) {
    this.#handle = handle;
  }

  async write(bytes: Uint8Array): Promise<void> {
    let offset = 0;
    while (offset < bytes.byteLength) {
      const result = await this.#handle.write(bytes, offset, bytes.byteLength - offset);
      if (result.bytesWritten <= 0) {
        throw new ModelManagerError('IO', 'Unable to write model data.');
      }
      offset += result.bytesWritten;
    }
  }

  sync(): Promise<void> {
    return this.#handle.sync();
  }

  close(): Promise<void> {
    return this.#handle.close();
  }
}

export async function assertSameFilesystem(first: string, second: string): Promise<void> {
  const [firstMetadata, secondMetadata] = await Promise.all([lstat(first), lstat(second)]);
  if (firstMetadata.dev !== secondMetadata.dev) {
    throw new ModelManagerError(
      'PROTOCOL',
      'Model staging must use the same filesystem as installed models.',
    );
  }
}

export async function ensureSafeDirectory(root: string, destination: string): Promise<void> {
  const absoluteRoot = resolve(root);
  const absoluteDestination = resolve(destination);
  const suffix = relative(absoluteRoot, absoluteDestination);
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  await mkdir(absoluteRoot, { recursive: true, mode: 0o700 });
  let current = absoluteRoot;
  await assertDirectoryNotLink(current);
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await mkdir(current, { mode: 0o700 });
    } catch (error: unknown) {
      if (!hasCode(error, 'EEXIST')) throw error;
    }
    await assertDirectoryNotLink(current);
  }
}

async function assertDirectoryNotLink(path: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new ModelManagerError('CORRUPT', 'Managed model directory contains a link.', true);
  }
}

/** Validates existing parents without turning a read-only status check into a filesystem mutation. */
export async function assertSafeExistingDirectoryChain(
  root: string,
  destination: string,
): Promise<void> {
  const absoluteRoot = resolve(root);
  const suffix = relative(absoluteRoot, resolve(destination));
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  let current = absoluteRoot;
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await assertDirectoryNotLink(current);
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) return;
      throw error;
    }
  }
}

export async function safeRegularFileSize(path: string): Promise<number> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new ModelManagerError('CORRUPT', 'Managed model file is not a regular file.', true);
    }
    return metadata.size;
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return 0;
    throw error;
  }
}

export async function verifiedIdentityStillCurrent(
  path: string,
  expected: Omit<VerifiedModelFileIdentity, 'path'>,
): Promise<boolean> {
  try {
    const metadata = await lstat(path);
    return (
      metadata.isFile() && !metadata.isSymbolicLink() && sameVerifiedIdentity(expected, metadata)
    );
  } catch {
    return false;
  }
}

export async function openSafePart(path: string, offset: number): Promise<FileHandle> {
  const flags =
    constants.O_WRONLY |
    constants.O_CREAT |
    constants.O_NOFOLLOW |
    (offset === 0 ? constants.O_TRUNC : constants.O_APPEND);
  const handle = await open(path, flags, 0o600);
  const metadata = await handle.stat();
  let pathMetadata: Stats;
  try {
    pathMetadata = await lstat(path);
  } catch (error: unknown) {
    await handle.close();
    throw error;
  }
  if (
    !metadata.isFile() ||
    metadata.size !== offset ||
    pathMetadata.isSymbolicLink() ||
    !pathMetadata.isFile() ||
    !sameOpenedFile(metadata, pathMetadata)
  ) {
    await handle.close();
    throw new ModelManagerError(
      'CORRUPT',
      'Download staging path changed during secure open.',
      true,
    );
  }
  return handle;
}

export function fileSystemIdentityKey(
  identity: Pick<VerifiedModelFileIdentity, 'device' | 'inode'>,
): string {
  return `${identity.device}:${identity.inode}`;
}

export function sameOpenedFile(first: Stats, second: Stats): boolean {
  return (
    first.size === second.size &&
    first.mtimeMs === second.mtimeMs &&
    first.ctimeMs === second.ctimeMs &&
    first.birthtimeMs === second.birthtimeMs &&
    first.dev === second.dev &&
    first.ino === second.ino
  );
}

export async function assertDownloadCapacity(
  remaining: number,
  availableBytes: () => Promise<number>,
): Promise<void> {
  const headroom = Math.max(
    MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
    Math.ceil(remaining * MODEL_DOWNLOAD_HEADROOM_RATIO),
  );
  if (remaining > 0 && (await availableBytes()) < remaining + headroom) {
    throw new ModelManagerError('DISK_SPACE', 'Not enough disk space to download this model.');
  }
}

export async function defaultAvailableBytes(path: string): Promise<number> {
  const values = await statfs(path);
  const available = BigInt(values.bavail) * BigInt(values.bsize);
  return available > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(available);
}

export function canDownloadInsteadOfLink(error: unknown): boolean {
  return ['EXDEV', 'EPERM', 'EACCES', 'ENOSYS', 'ENOTSUP', 'EOPNOTSUPP', 'EMLINK', 'ENOENT'].some(
    (code) => hasCode(error, code),
  );
}

export async function removeObsoleteRevisions(
  parent: string,
  currentRevision: string,
): Promise<void> {
  const entries = await readdir(parent, { withFileTypes: true });
  await Promise.all(
    entries
      .filter(
        (entry) =>
          entry.name !== currentRevision &&
          /^[a-f0-9]{40}$/u.test(entry.name) &&
          entry.isDirectory() &&
          !entry.isSymbolicLink(),
      )
      .map((entry) => rm(join(parent, entry.name), { recursive: true, force: true })),
  );
}

export function hasCode(error: unknown, code: string): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { readonly code?: unknown }).code === code
  );
}
