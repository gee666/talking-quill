import { randomBytes } from 'node:crypto';
import { lstat, mkdir, mkdtemp, readFile, realpath, rm, writeFile } from 'node:fs/promises';
import { isAbsolute, relative, resolve } from 'node:path';

const temporaryRoot = resolve('tmp', 'tests');
const markerName = '.talking-quill-test-owner.json';
const ownedDirectories = new Map<string, { readonly token: string; readonly root: string }>();

export async function createTestDirectory(prefix: string): Promise<string> {
  if (!/^[a-z0-9][a-z0-9-]{0,63}$/u.test(prefix)) {
    throw new Error(`Unsafe test-directory prefix: ${prefix}`);
  }
  await mkdir(temporaryRoot, { recursive: true });
  await assertOrdinaryDirectory(temporaryRoot, 'Test temporary root');
  const canonicalBase = await realpath(temporaryRoot);
  const created = await mkdtemp(resolve(canonicalBase, `${prefix}-`));
  await assertOrdinaryDirectory(created, 'Owned test directory');
  const canonicalRoot = await realpath(created);
  assertContained(canonicalBase, canonicalRoot);
  const token = randomBytes(32).toString('hex');
  await writeFile(
    resolve(canonicalRoot, markerName),
    `${JSON.stringify({ schemaVersion: 1, kind: 'talking-quill-test-directory', token, pid: process.pid, root: canonicalRoot })}\n`,
    { encoding: 'utf8', flag: 'wx' },
  );
  ownedDirectories.set(canonicalRoot, { token, root: canonicalRoot });
  return canonicalRoot;
}

export async function removeTestDirectory(path: string): Promise<void> {
  const canonicalBase = await realpath(temporaryRoot);
  const registered = ownedDirectories.get(path);
  if (registered === undefined)
    throw new Error(`Refusing to remove an unowned test directory: ${path}`);
  await assertOrdinaryDirectory(path, 'Owned test directory');
  const canonicalRoot = await realpath(path);
  if (canonicalRoot !== registered.root || canonicalRoot !== path) {
    throw new Error('Owned test directory canonical path changed');
  }
  assertContained(canonicalBase, canonicalRoot);
  const markerPath = resolve(canonicalRoot, markerName);
  const markerMetadata = await lstat(markerPath);
  if (!markerMetadata.isFile() || markerMetadata.isSymbolicLink()) {
    throw new Error('Test-directory ownership marker is not a regular file');
  }
  const marker = JSON.parse(await readFile(markerPath, 'utf8')) as Record<string, unknown>;
  if (
    marker.schemaVersion !== 1 ||
    marker.kind !== 'talking-quill-test-directory' ||
    marker.token !== registered.token ||
    marker.pid !== process.pid ||
    marker.root !== canonicalRoot
  ) {
    throw new Error('Test-directory ownership marker mismatch');
  }
  await rm(canonicalRoot, { recursive: true, force: false, maxRetries: 3 });
  try {
    await lstat(canonicalRoot);
    throw new Error('Owned test directory remains after cleanup');
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code !== 'ENOENT') throw error;
  }
  ownedDirectories.delete(canonicalRoot);
}

async function assertOrdinaryDirectory(path: string, description: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new Error(`${description} must be an ordinary directory, not a reparse point`);
  }
}

function assertContained(base: string, candidate: string): void {
  const result = relative(base, candidate);
  if (result === '' || /^\.\.(?:[\\/]|$)/u.test(result) || isAbsolute(result)) {
    throw new Error(`Test directory escapes or equals its owned root: ${candidate}`);
  }
}
