import { createHash, randomUUID } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import { link, lstat, mkdir, open, readFile, rename, rm } from 'node:fs/promises';
import { dirname } from 'node:path';
import writeFileAtomic from 'write-file-atomic';

export async function readUtf8File(path: string): Promise<string | null> {
  try {
    return await readFile(path, 'utf8');
  } catch (error: unknown) {
    if (isNodeError(error) && error.code === 'ENOENT') return null;
    throw error;
  }
}

export async function writeJsonAtomic(path: string, value: unknown): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  await writeFileAtomic(path, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: 'utf8',
    mode: 0o600,
    fsync: true,
  });
  await syncDirectory(dirname(path));
}

/** Persists directory-entry publication/removal, including write-through directory handles. */
export async function syncDirectory(path: string): Promise<void> {
  const handle = await open(path, 'r');
  try {
    await handle.sync().catch((error: unknown) => {
      // libuv does not expose FlushFileBuffers for directory handles on Windows. The file itself
      // is fsynced above; callers that require reset-journal durability retain the journal and
      // fail closed when later identity/state validation is ambiguous after a power loss.
      if (process.platform !== 'win32' || !isNodeError(error) || error.code !== 'EPERM')
        throw error;
    });
  } finally {
    await handle.close();
  }
}

export class ConcurrentFileReplacementError extends Error {
  constructor(path: string) {
    super(`Persistence file changed before invalid-data quarantine: ${path}`);
    this.name = 'ConcurrentFileReplacementError';
  }
}

export async function preserveInvalidFile(
  path: string,
  previouslyReadSource?: string,
): Promise<string | null> {
  const claimedPath = `${path}.${randomUUID()}.invalid-source`;
  try {
    await rename(path, claimedPath);
  } catch (error: unknown) {
    if (isNodeError(error) && error.code === 'ENOENT') return null;
    throw error;
  }

  let source: Buffer;
  try {
    source = await readClaimedRegularFile(claimedPath);
  } catch (error: unknown) {
    await restoreClaimedFile(claimedPath, path);
    throw error;
  }

  try {
    if (previouslyReadSource !== undefined && source.toString('utf8') !== previouslyReadSource) {
      await restoreClaimedFile(claimedPath, path);
      throw new ConcurrentFileReplacementError(path);
    }

    const timestamp = new Date().toISOString().replaceAll(':', '-');
    const destination = `${path}.${timestamp}.invalid`;
    const quarantine = Object.freeze({
      quarantinedAt: new Date().toISOString(),
      byteLength: source.byteLength,
      sha256: createHash('sha256').update(source).digest('hex'),
    });
    // The atomic claim above makes the destructive operation identity-safe: a replacement
    // published at the original path is never addressed by this removal.
    await rm(claimedPath);
    await writeJsonAtomic(destination, quarantine);
    return destination;
  } finally {
    source.fill(0);
  }
}

async function readClaimedRegularFile(path: string): Promise<Buffer> {
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const [opened, pathBefore] = await Promise.all([handle.stat(), lstat(path)]);
    if (
      !opened.isFile() ||
      pathBefore.isSymbolicLink() ||
      !pathBefore.isFile() ||
      !sameFileIdentity(opened, pathBefore)
    ) {
      throw new Error('Claimed persistence source is not a stable regular file.');
    }
    const source = await handle.readFile();
    const [afterHandle, pathAfter] = await Promise.all([handle.stat(), lstat(path)]);
    if (!sameFileIdentity(opened, afterHandle) || !sameFileIdentity(afterHandle, pathAfter)) {
      source.fill(0);
      throw new Error('Claimed persistence source changed while it was being read.');
    }
    return source;
  } finally {
    await handle.close();
  }
}

async function restoreClaimedFile(claimedPath: string, path: string): Promise<void> {
  try {
    // link() is a no-clobber restore: unlike rename(), it cannot overwrite a newer replacement.
    await link(claimedPath, path);
  } catch (error: unknown) {
    if (!isNodeError(error) || error.code !== 'EEXIST') throw error;
  }
  await rm(claimedPath);
}

function sameFileIdentity(first: Stats, second: Stats): boolean {
  return (
    first.dev === second.dev &&
    first.ino === second.ino &&
    first.size === second.size &&
    first.mtimeMs === second.mtimeMs &&
    first.ctimeMs === second.ctimeMs &&
    first.birthtimeMs === second.birthtimeMs
  );
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
