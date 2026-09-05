import { createHash } from 'node:crypto';
import type { BigIntStats } from 'node:fs';
import { homedir } from 'node:os';
import { lstat, realpath, rename } from 'node:fs/promises';
import { dirname, isAbsolute, parse, relative, resolve, sep } from 'node:path';
import { z } from 'zod';
import { APP_OWNERSHIP_ID, type ResetJournal } from './reset-journal';

const OWNERSHIP_MARKER_VERSION = 1 as const;
export const OwnershipMarkerSchema = z
  .object({
    schemaVersion: z.literal(OWNERSHIP_MARKER_VERSION),
    appId: z.literal(APP_OWNERSHIP_ID),
    rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
  })
  .strict();
// Path validation does not grant deletion authority. Recovery must still verify file identity.
export class OwnedDataLocation {
  readonly root: string;
  readonly #allowedBase: string;
  readonly #home: string;

  constructor(userDataRoot: string, allowedBase: string, homeDirectory?: string) {
    this.root = validateUserDataRoot(userDataRoot);
    this.#allowedBase = validateUserDataRoot(allowedBase, true);
    this.#home = resolve(homeDirectory ?? homedir());
    assertNotHomeOrProfileAncestor(this.root, this.#home);
    assertLexicallyContained(this.#allowedBase, this.root);
  }

  validateJournalBinding(journal: ResetJournal): {
    tombstonePath: string;
    disposalPath: string;
  } {
    if (resolve(journal.userDataRoot) !== this.root) {
      throw new Error('Reset journal does not match the application data root');
    }
    const tombstonePath = resetTombstonePath(this.root, journal.rootIdentity, journal.nonce);
    const disposalPath = resetDisposalPath(this.root, journal.rootIdentity, journal.nonce);
    if (
      resolve(journal.tombstonePath) !== tombstonePath ||
      resolve(journal.disposalPath) !== disposalPath ||
      dirname(tombstonePath) !== dirname(this.root) ||
      dirname(disposalPath) !== dirname(this.root) ||
      tombstonePath === disposalPath
    ) {
      throw new Error('Reset journal tombstone or disposal binding is invalid');
    }
    return { tombstonePath, disposalPath };
  }

  async assertCanonicalOwnedLocation(): Promise<string> {
    const rootMetadata = await lstat(this.root);
    if (rootMetadata.isSymbolicLink()) {
      throw new Error('Refusing a symbolic-link or junction application data root');
    }
    const [canonicalBase, canonicalRoot, canonicalHome] = await Promise.all([
      realpath(this.#allowedBase),
      realpath(this.root),
      realpath(this.#home).catch(() => this.#home),
    ]);
    assertCanonicallyContained(canonicalBase, canonicalRoot);
    assertNotHomeOrProfileAncestor(canonicalRoot, canonicalHome);
    return canonicalRoot;
  }

  async restoreUnverifiedRename(
    sourcePath: string,
    destinationPath: string,
    movedMetadata: BigIntStats,
  ): Promise<void> {
    if (movedMetadata.isSymbolicLink() || !movedMetadata.isDirectory()) return;
    if ((await lstatOrNull(destinationPath)) !== null || process.platform !== 'win32') return;
    const currentTombstone = await lstatOrNull(sourcePath);
    if (
      currentTombstone === null ||
      currentTombstone.isSymbolicLink() ||
      !currentTombstone.isDirectory() ||
      fileIdentity(currentTombstone) !== fileIdentity(movedMetadata)
    ) {
      return;
    }
    try {
      // rename is intentionally attempted only with a vacant destination. On Windows an occupied
      // destination fails rather than replacing it; either outcome retains the journal.
      await rename(sourcePath, destinationPath);
      const restored = await lstatOrNull(destinationPath);
      if (
        restored === null ||
        restored.isSymbolicLink() ||
        fileIdentity(restored) !== fileIdentity(movedMetadata)
      ) {
        return;
      }
    } catch {
      // Preserve the exact moved directory as a journal-bound quarantine for manual recovery.
    }
  }

  async assertSafeResetDirectory(
    path: string,
    metadata: BigIntStats,
    expectedFileIdentity: string,
    kind: 'tombstone' | 'disposal',
  ): Promise<void> {
    if (
      !metadata.isDirectory() ||
      metadata.isSymbolicLink() ||
      fileIdentity(metadata) !== expectedFileIdentity
    ) {
      throw new Error(`Reset ${kind} is not the journal-recorded directory`);
    }
    const [canonicalBase, canonicalParent, canonicalTombstone] = await Promise.all([
      realpath(this.#allowedBase),
      realpath(dirname(path)),
      realpath(path),
    ]);
    assertCanonicalParentContained(canonicalBase, canonicalParent);
    if (dirname(canonicalTombstone) !== canonicalParent) {
      throw new Error(`Reset ${kind} escaped its journal-bound sibling directory`);
    }
  }
}

export function resetJournalPath(userDataRoot: string): string {
  const root = validateUserDataRoot(userDataRoot);
  const identity = createHash('sha256').update(root).digest('hex').slice(0, 24);
  return resolve(dirname(root), `.talking-quill-reset-${identity}.json`);
}

export function resetTombstonePath(userDataRoot: string, identity: string, nonce: string): string {
  return resetSiblingPath('tombstone', userDataRoot, identity, nonce);
}

export function resetDisposalPath(userDataRoot: string, identity: string, nonce: string): string {
  return resetSiblingPath('disposal', userDataRoot, identity, nonce);
}

function resetSiblingPath(
  kind: 'tombstone' | 'disposal',
  userDataRoot: string,
  identity: string,
  nonce: string,
): string {
  if (!/^[a-f0-9]{64}$/u.test(identity) || !z.uuid().safeParse(nonce).success) {
    throw new Error(`Reset ${kind} identity is invalid`);
  }
  const root = validateUserDataRoot(userDataRoot);
  return resolve(
    dirname(root),
    `.talking-quill-reset-${kind}-${identity.slice(0, 24)}-${nonce.toLowerCase()}`,
  );
}

export function validateUserDataRoot(userDataRoot: string, allowProfileBase = false): string {
  const root = resolve(userDataRoot);
  const parsed = parse(root);
  if (root === parsed.root || dirname(root) === root || (!allowProfileBase && root === homedir())) {
    throw new Error('Refusing to manage an unsafe application data root');
  }
  return root;
}

export function ownershipMarker(canonicalRoot: string): z.infer<typeof OwnershipMarkerSchema> {
  return {
    schemaVersion: OWNERSHIP_MARKER_VERSION,
    appId: APP_OWNERSHIP_ID,
    rootIdentity: rootIdentity(canonicalRoot),
  };
}

export function rootIdentity(canonicalRoot: string): string {
  return createHash('sha256').update(`${APP_OWNERSHIP_ID}\0${canonicalRoot}`).digest('hex');
}

export function fileIdentity(metadata: BigIntStats): string {
  return `${String(metadata.dev)}:${String(metadata.ino)}`;
}

export async function lstatOrNull(path: string): Promise<BigIntStats | null> {
  try {
    return await lstat(path, { bigint: true });
  } catch (error: unknown) {
    if (isNodeError(error) && error.code === 'ENOENT') return null;
    throw error;
  }
}

function assertLexicallyContained(base: string, candidate: string): void {
  const path = relative(base, candidate);
  if (path.length === 0 || path === '..' || path.startsWith(`..${sep}`) || isAbsolute(path)) {
    throw new Error('Application data root is outside its allowed base');
  }
}

function assertCanonicallyContained(base: string, candidate: string): void {
  const path = relative(base, candidate);
  if (
    path.length === 0 ||
    path === '..' ||
    path.startsWith('../') ||
    path.startsWith('..\\') ||
    isAbsolute(path)
  ) {
    throw new Error('Canonical application data root is outside its allowed base');
  }
}

function assertCanonicalParentContained(base: string, candidate: string): void {
  const path = relative(base, candidate);
  if (path === '..' || path.startsWith('../') || path.startsWith('..\\') || isAbsolute(path)) {
    throw new Error('Reset tombstone parent is outside its allowed base');
  }
}

function assertNotHomeOrProfileAncestor(candidate: string, home: string): void {
  const homeFromCandidate = relative(candidate, home);
  if (
    homeFromCandidate.length === 0 ||
    (!homeFromCandidate.startsWith('../') &&
      !homeFromCandidate.startsWith('..\\') &&
      !isAbsolute(homeFromCandidate))
  ) {
    throw new Error('Refusing to manage a home or profile ancestor');
  }
}

export function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
