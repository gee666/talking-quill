import { createHash } from 'node:crypto';
import { mkdir, open, readFile, rm } from 'node:fs/promises';
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

export async function preserveInvalidFile(path: string): Promise<string | null> {
  let source: Buffer;
  try {
    source = await readFile(path);
  } catch (error: unknown) {
    if (isNodeError(error) && error.code === 'ENOENT') return null;
    throw error;
  }

  const timestamp = new Date().toISOString().replaceAll(':', '-');
  const destination = `${path}.${timestamp}.invalid`;
  const quarantine = Object.freeze({
    quarantinedAt: new Date().toISOString(),
    byteLength: source.byteLength,
    sha256: createHash('sha256').update(source).digest('hex'),
  });
  // Never retain malformed user data: it may contain plaintext secrets in otherwise
  // unparseable JSON. Only non-reversible diagnostics are written to quarantine.
  await rm(path, { force: true });
  await writeJsonAtomic(destination, quarantine);
  source.fill(0);
  return destination;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
