import type { Dirent } from 'node:fs';
import { lstat, mkdir, opendir, realpath, rm } from 'node:fs/promises';
import { basename, dirname, join, parse, resolve } from 'node:path';

export async function scavengeSessionArtifacts(
  directory: string,
  batchSize = 64,
  signal?: AbortSignal,
): Promise<void> {
  if (!Number.isSafeInteger(batchSize) || batchSize < 1)
    throw new Error('Session cleanup batch size is invalid');
  if (isAborted(signal)) return;

  const cleanupRoot = await prepareCleanupRoot(directory, signal);
  if (cleanupRoot === null) return;
  await revalidateCleanupRoot(cleanupRoot);

  const handle = await opendir(cleanupRoot.directory);
  let batch: Dirent[] = [];
  for await (const entry of handle) {
    if (isAborted(signal)) return;
    batch.push(entry);
    if (batch.length < batchSize) continue;
    await revalidateCleanupRoot(cleanupRoot);
    await removeBatch(cleanupRoot.directory, batch);
    batch = [];
    await new Promise<void>((resolveWait) => setImmediate(resolveWait));
  }
  if (isAborted(signal) || batch.length === 0) return;
  await revalidateCleanupRoot(cleanupRoot);
  await removeBatch(cleanupRoot.directory, batch);
}

function isAborted(signal?: AbortSignal): boolean {
  return signal?.aborted === true;
}

interface DirectoryIdentity {
  readonly canonicalPath: string;
  readonly device: number;
  readonly inode: number;
}

interface CleanupRootIdentity {
  readonly directory: string;
  readonly parent: DirectoryIdentity;
  readonly leaf: DirectoryIdentity;
}

async function prepareCleanupRoot(
  directory: string,
  signal?: AbortSignal,
): Promise<CleanupRootIdentity | null> {
  const requestedDirectory = resolve(directory);
  const requestedParent = dirname(requestedDirectory);
  if (
    requestedParent === requestedDirectory ||
    requestedParent === parse(requestedDirectory).root
  ) {
    throw new Error('Session cleanup cannot target a filesystem root or its direct child');
  }
  if (isAborted(signal)) return null;

  const parent = await readDirectoryIdentity(requestedParent, 'parent');
  if (isAborted(signal)) return null;
  const canonicalLeafPath = join(parent.canonicalPath, basename(requestedDirectory));
  let leaf: DirectoryIdentity;
  try {
    leaf = await readDirectoryIdentity(canonicalLeafPath, 'root');
  } catch (error: unknown) {
    if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    if (isAborted(signal)) return null;
    try {
      await mkdir(canonicalLeafPath, { recursive: false, mode: 0o700 });
    } catch (mkdirError: unknown) {
      if (!isNodeError(mkdirError) || mkdirError.code !== 'EEXIST') throw mkdirError;
    }
    leaf = await readDirectoryIdentity(canonicalLeafPath, 'root');
  }
  assertDirectChild(parent, leaf);
  assertSameIdentity(parent, await readDirectoryIdentity(parent.canonicalPath, 'parent'));
  return { directory: leaf.canonicalPath, parent, leaf };
}

async function revalidateCleanupRoot(root: CleanupRootIdentity): Promise<void> {
  const parent = await readDirectoryIdentity(root.parent.canonicalPath, 'parent');
  const leaf = await readDirectoryIdentity(root.directory, 'root');
  assertSameIdentity(root.parent, parent);
  assertSameIdentity(root.leaf, leaf);
  assertDirectChild(parent, leaf);
}

async function readDirectoryIdentity(
  directory: string,
  description: 'parent' | 'root',
): Promise<DirectoryIdentity> {
  const metadata = await lstat(directory);
  if (metadata.isSymbolicLink() || !metadata.isDirectory()) {
    throw new Error(`Session cleanup ${description} must be a direct, owned directory`);
  }
  return {
    canonicalPath: await realpath(directory),
    device: metadata.dev,
    inode: metadata.ino,
  };
}

function assertDirectChild(parent: DirectoryIdentity, leaf: DirectoryIdentity): void {
  if (dirname(leaf.canonicalPath) !== parent.canonicalPath) {
    throw new Error('Session cleanup root must remain a direct child of its canonical parent');
  }
}

function assertSameIdentity(expected: DirectoryIdentity, actual: DirectoryIdentity): void {
  const identityAvailable = expected.device !== 0 || expected.inode !== 0;
  if (
    expected.canonicalPath !== actual.canonicalPath ||
    (identityAvailable && (expected.device !== actual.device || expected.inode !== actual.inode))
  ) {
    throw new Error('Session cleanup directory identity changed before removal');
  }
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}

async function removeBatch(directory: string, entries: readonly Dirent[]): Promise<void> {
  await Promise.all(
    entries.map((entry) =>
      rm(join(directory, entry.name), {
        recursive: entry.isDirectory(),
        force: true,
        maxRetries: 3,
      }),
    ),
  );
}
