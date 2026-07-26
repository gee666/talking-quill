import { randomUUID } from 'node:crypto';
import { type Stats } from 'node:fs';
import { lstat, readdir, rename, rm } from 'node:fs/promises';
import { basename, dirname, join } from 'node:path';

export type RevisionBackupRemover = (path: string) => Promise<void>;

export async function publishStagedFile(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  await renameWithWindowsRetry(source, target, renameOperation);
}

export async function publishRevisionDirectory(
  source: string,
  target: string,
  renameOperation: typeof rename,
  removeBackup: RevisionBackupRemover = defaultRemoveRevisionBackup,
): Promise<void> {
  await recoverRevisionDirectory(target, renameOperation, removeBackup);
  const backup = `${target}.${randomUUID()}.replaced`;
  let movedExisting = false;
  try {
    try {
      await renameWithWindowsRetry(target, backup, renameOperation);
      movedExisting = true;
    } catch (error: unknown) {
      if (!hasCode(error, 'ENOENT')) throw error;
    }
    await renameWithWindowsRetry(source, target, renameOperation);
  } catch (error: unknown) {
    await renameWithWindowsRetry(backup, target, renameOperation).catch(() => undefined);
    throw error;
  }
  // Reused repair files may be hard links into the replaced directory. Their ctime is not stable
  // until every obsolete link is removed, so cleanup must finish before identity publication.
  if (movedExisting) await removeBackup(backup);
}

export async function publishAtomically(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  if (process.platform !== 'win32') {
    await renameOperation(source, target);
    return;
  }
  await recoverReplacementTarget(target, 'file', renameOperation);
  const backup = `${target}.${randomUUID()}.replaced`;
  let movedExisting = false;
  try {
    try {
      await renameWithWindowsRetry(target, backup, renameOperation);
      movedExisting = true;
    } catch (error: unknown) {
      if (!hasCode(error, 'ENOENT')) throw error;
    }
    await renameWithWindowsRetry(source, target, renameOperation);
  } catch (error: unknown) {
    await renameWithWindowsRetry(backup, target, renameOperation).catch(() => undefined);
    throw error;
  }
  // Publication already succeeded. A scanner holding the obsolete backup must not turn success
  // into a false setup failure; startup recovery safely removes it later.
  if (movedExisting) await rm(backup, { force: true }).catch(() => undefined);
}

async function renameWithWindowsRetry(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  const attempts = process.platform === 'win32' ? 7 : 1;
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      await renameOperation(source, target);
      return;
    } catch (error: unknown) {
      lastError = error;
      if (!isTransientWindowsFileError(error) || attempt + 1 === attempts) throw error;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 25 * 2 ** attempt));
    }
  }
  throw lastError;
}

export function recoverRevisionDirectory(
  target: string,
  renameOperation: typeof rename = rename,
  removeBackup: RevisionBackupRemover = defaultRemoveRevisionBackup,
): Promise<void> {
  return recoverReplacementTarget(target, 'directory', renameOperation, removeBackup);
}

async function recoverReplacementTarget(
  target: string,
  expectedType: 'file' | 'directory',
  renameOperation: typeof rename,
  removeRevisionBackup?: RevisionBackupRemover,
): Promise<void> {
  const directory = dirname(target);
  const name = basename(target);
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return;
    throw error;
  }
  const backups = entries
    .filter(
      (entry) =>
        entry.name === `${name}.replaced` ||
        (entry.name.startsWith(`${name}.`) && entry.name.endsWith('.replaced')),
    )
    .map((entry) => join(directory, entry.name))
    .sort();
  if (backups.length === 0) return;
  const targetMetadata = await safeLstat(target);
  if (targetMetadata !== null) {
    await Promise.all(
      backups.map((backup) =>
        removeRevisionBackup === undefined
          ? rm(backup, { recursive: true, force: true }).catch(() => undefined)
          : removeRevisionBackup(backup),
      ),
    );
    return;
  }
  let restored = false;
  for (const backup of backups) {
    const metadata = await safeLstat(backup);
    const validReplacement =
      metadata !== null &&
      !metadata.isSymbolicLink() &&
      (expectedType === 'file' ? metadata.isFile() : metadata.isDirectory());
    if (!restored && validReplacement) {
      await renameOperation(backup, target);
      restored = true;
    } else if (removeRevisionBackup === undefined) {
      await rm(backup, { recursive: true, force: true });
    } else {
      await removeRevisionBackup(backup);
    }
  }
}

function defaultRemoveRevisionBackup(path: string): Promise<void> {
  return rm(path, { recursive: true, force: true });
}

async function safeLstat(path: string): Promise<Stats | null> {
  try {
    return await lstat(path);
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return null;
    throw error;
  }
}

function isTransientWindowsFileError(error: unknown): boolean {
  return (
    process.platform === 'win32' &&
    ['EBUSY', 'EPERM', 'EACCES'].some((code) => hasCode(error, code))
  );
}

function hasCode(error: unknown, code: string): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { readonly code?: unknown }).code === code
  );
}
