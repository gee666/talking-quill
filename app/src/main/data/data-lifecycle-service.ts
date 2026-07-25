import { createHash, randomUUID } from 'node:crypto';
import type { BigIntStats } from 'node:fs';
import { homedir } from 'node:os';
import { lstat, readFile, realpath, rename, rm } from 'node:fs/promises';
import { dirname, isAbsolute, parse, relative, resolve, sep } from 'node:path';
import { z } from 'zod';
import { syncDirectory, writeJsonAtomic } from '../persistence/atomic-json';

const RESET_JOURNAL_VERSION = 4 as const;
const OWNERSHIP_MARKER_VERSION = 1 as const;
const APP_OWNERSHIP_ID = 'com.talkingquill.app' as const;
const OwnershipMarkerSchema = z
  .object({
    schemaVersion: z.literal(OWNERSHIP_MARKER_VERSION),
    appId: z.literal(APP_OWNERSHIP_ID),
    rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
  })
  .strict();
const LegacyV2ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(2),
    appId: z.literal(APP_OWNERSHIP_ID),
    userDataRoot: z.string().min(1).max(32_768),
    rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
    requestedAt: z.number().int().nonnegative(),
    nonce: z.uuid(),
  })
  .strict();
const LegacyV3ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(3),
    appId: z.literal(APP_OWNERSHIP_ID),
    userDataRoot: z.string().min(1).max(32_768),
    rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
    rootFileIdentity: z.string().regex(/^\d+:\d+$/),
    tombstonePath: z.string().min(1).max(32_768),
    requestedAt: z.number().int().nonnegative(),
    nonce: z.uuid(),
  })
  .strict();
const ResetJournalSchema = z
  .object({
    schemaVersion: z.literal(RESET_JOURNAL_VERSION),
    appId: z.literal(APP_OWNERSHIP_ID),
    userDataRoot: z.string().min(1).max(32_768),
    rootIdentity: z.string().regex(/^[a-f0-9]{64}$/),
    rootFileIdentity: z.string().regex(/^\d+:\d+$/),
    tombstonePath: z.string().min(1).max(32_768),
    disposalPath: z.string().min(1).max(32_768),
    phase: z.enum(['rename-pending', 'disposal-pending']),
    requestedAt: z.number().int().nonnegative(),
    nonce: z.uuid(),
  })
  .strict();

export type ResetFaultPhase =
  | 'after-journal-write'
  | 'before-live-rename'
  | 'before-renamed-identity-check'
  | 'after-live-rename'
  | 'before-tombstone-remove'
  | 'before-tombstone-disposal-transition'
  | 'after-tombstone-disposal-transition'
  | 'before-disposal-remove'
  | 'before-identity-bound-remove'
  | 'after-tombstone-remove'
  | 'before-journal-remove'
  | 'after-journal-remove';

export interface IdentityBoundRemovalRequest {
  readonly path: string;
  readonly expectedFileIdentity: string;
}

export interface DataLifecycleOptions {
  readonly allowedBase: string;
  readonly homeDirectory?: string;
  /**
   * Privileged boundary that must bind recursive deletion to expectedFileIdentity rather than
   * resolving path after validation. Omission deliberately makes destructive reset fail closed.
   */
  readonly removeIdentityBoundDirectory?: (request: IdentityBoundRemovalRequest) => Promise<void>;
  /** Deterministic durability fault injection; production uses the real directory fsync. */
  readonly syncResetDirectory?: (path: string) => Promise<void>;
  /** Deterministic fault injection for lifecycle tests. Production callers leave this undefined. */
  readonly writeResetJournal?: (path: string, value: unknown) => Promise<void>;
  readonly injectResetFault?: (phase: ResetFaultPhase) => Promise<void>;
}

export interface ResetRecoveryResult {
  readonly recovered: boolean;
}

export class DataLifecycleService {
  readonly #root: string;
  readonly #allowedBase: string;
  readonly #home: string;
  readonly #journalPath: string;
  readonly #markerPath: string;
  readonly #writeResetJournal: (path: string, value: unknown) => Promise<void>;
  readonly #injectResetFault: (phase: ResetFaultPhase) => Promise<void>;
  readonly #removeIdentityBoundDirectory: (request: IdentityBoundRemovalRequest) => Promise<void>;
  readonly #syncResetDirectory: (path: string) => Promise<void>;
  readonly #canPrepareDestructiveReset: boolean;
  #prepared = false;

  constructor(userDataRoot: string, options: DataLifecycleOptions) {
    this.#root = validateUserDataRoot(userDataRoot);
    this.#allowedBase = validateUserDataRoot(options.allowedBase, true);
    this.#home = resolve(options.homeDirectory ?? homedir());
    assertNotHomeOrProfileAncestor(this.#root, this.#home);
    assertLexicallyContained(this.#allowedBase, this.#root);
    this.#journalPath = resetJournalPath(this.#root);
    this.#markerPath = resolve(this.#root, '.talking-quill-owner.json');
    this.#writeResetJournal = options.writeResetJournal ?? writeJsonAtomic;
    this.#injectResetFault = options.injectResetFault ?? (() => Promise.resolve());
    this.#canPrepareDestructiveReset = options.removeIdentityBoundDirectory !== undefined;
    this.#removeIdentityBoundDirectory =
      options.removeIdentityBoundDirectory ?? failClosedIdentityBoundRemoval;
    this.#syncResetDirectory = options.syncResetDirectory ?? syncDirectory;
  }

  get journalPath(): string {
    return this.#journalPath;
  }

  get markerPath(): string {
    return this.#markerPath;
  }

  async initializeOwnership(): Promise<void> {
    const canonicalRoot = await this.#assertCanonicalOwnedLocation();
    const expected = ownershipMarker(canonicalRoot);
    let source: string | null = null;
    try {
      source = await readFile(this.#markerPath, 'utf8');
    } catch (error: unknown) {
      if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    }
    if (source === null) {
      await writeJsonAtomic(this.#markerPath, expected);
      return;
    }
    const marker = OwnershipMarkerSchema.parse(JSON.parse(source) as unknown);
    if (marker.rootIdentity !== expected.rootIdentity) {
      throw new Error('Application data ownership marker does not match its canonical root');
    }
  }

  async recoverPendingReset(): Promise<ResetRecoveryResult> {
    let journal = await this.#readJournal();
    if (journal === null) return { recovered: false };
    const { tombstonePath, disposalPath } = this.#validateJournalBinding(journal);
    const [liveMetadata, tombstoneMetadata, disposalMetadata] = await Promise.all([
      lstatOrNull(this.#root),
      lstatOrNull(tombstonePath),
      lstatOrNull(disposalPath),
    ]);
    if ([liveMetadata, tombstoneMetadata, disposalMetadata].filter(Boolean).length > 1) {
      throw new Error(
        'Reset recovery is ambiguous because multiple live, tombstone, or disposal roots exist',
      );
    }
    if (
      (journal.phase === 'disposal-pending' && liveMetadata !== null) ||
      (journal.phase === 'rename-pending' && disposalMetadata !== null)
    ) {
      throw new Error('Reset journal phase does not match its live filesystem state');
    }

    if (liveMetadata !== null) {
      if (liveMetadata.isSymbolicLink()) {
        throw new Error('Refusing a symbolic-link or junction application data root');
      }
      const canonicalRoot = await this.#assertCanonicalOwnedLocation();
      const marker = await this.#readOwnershipMarker(this.#markerPath);
      const expectedIdentity = rootIdentity(canonicalRoot);
      if (
        marker.rootIdentity !== expectedIdentity ||
        journal.rootIdentity !== expectedIdentity ||
        journal.rootFileIdentity !== fileIdentity(liveMetadata)
      ) {
        throw new Error('Reset journal ownership could not be verified');
      }
      await this.#injectResetFault('before-live-rename');
      await this.#durableRename(this.#root, tombstonePath);
      await this.#injectResetFault('before-renamed-identity-check');
      const renamedMetadata = await lstat(tombstonePath, { bigint: true });
      if (
        renamedMetadata.isSymbolicLink() ||
        fileIdentity(renamedMetadata) !== journal.rootFileIdentity
      ) {
        await this.#restoreUnverifiedRename(tombstonePath, this.#root, renamedMetadata);
        throw new Error(
          'Application data root changed during atomic reset rename; reset remains quarantined',
        );
      }
      await this.#injectResetFault('after-live-rename');
    } else if (tombstoneMetadata !== null) {
      await this.#assertSafeResetDirectory(
        tombstonePath,
        tombstoneMetadata,
        journal.rootFileIdentity,
        'tombstone',
      );
    } else if (disposalMetadata !== null) {
      await this.#assertSafeResetDirectory(
        disposalPath,
        disposalMetadata,
        journal.rootFileIdentity,
        'disposal',
      );
    }

    const currentTombstone = await lstatOrNull(tombstonePath);
    if (currentTombstone !== null) {
      await this.#assertSafeResetDirectory(
        tombstonePath,
        currentTombstone,
        journal.rootFileIdentity,
        'tombstone',
      );
      await this.#injectResetFault('before-tombstone-remove');
      const transitionMetadata = await lstatOrNull(tombstonePath);
      if (transitionMetadata === null) {
        throw new Error('Reset tombstone disappeared before disposal transition');
      }
      await this.#assertSafeResetDirectory(
        tombstonePath,
        transitionMetadata,
        journal.rootFileIdentity,
        'tombstone',
      );
      if (journal.phase !== 'disposal-pending') {
        journal = { ...journal, phase: 'disposal-pending' };
        await this.#writeResetJournal(this.#journalPath, journal);
      }
      await this.#injectResetFault('before-tombstone-disposal-transition');
      await this.#durableRename(tombstonePath, disposalPath);
      await this.#injectResetFault('after-tombstone-disposal-transition');
      const transitionedMetadata = await lstat(disposalPath, { bigint: true });
      if (
        transitionedMetadata.isSymbolicLink() ||
        !transitionedMetadata.isDirectory() ||
        fileIdentity(transitionedMetadata) !== journal.rootFileIdentity
      ) {
        await this.#restoreUnverifiedRename(disposalPath, tombstonePath, transitionedMetadata);
        throw new Error(
          'Reset tombstone changed during atomic disposal transition; reset remains quarantined',
        );
      }
    }

    const currentDisposal = await lstatOrNull(disposalPath);
    if (currentDisposal !== null) {
      await this.#assertSafeResetDirectory(
        disposalPath,
        currentDisposal,
        journal.rootFileIdentity,
        'disposal',
      );
      await this.#injectResetFault('before-disposal-remove');
      const deletionMetadata = await lstatOrNull(disposalPath);
      if (deletionMetadata === null) {
        throw new Error('Reset disposal directory disappeared before deletion');
      }
      await this.#assertSafeResetDirectory(
        disposalPath,
        deletionMetadata,
        journal.rootFileIdentity,
        'disposal',
      );
      await this.#injectResetFault('before-identity-bound-remove');
      await this.#removeIdentityBoundDirectory({
        path: disposalPath,
        expectedFileIdentity: journal.rootFileIdentity,
      });
      if ((await lstatOrNull(disposalPath)) !== null) {
        throw new Error('Identity-bound reset boundary did not remove the recorded directory');
      }
      await this.#syncResetDirectory(dirname(disposalPath));
      await this.#injectResetFault('after-tombstone-remove');
    }
    const finalEntries = await Promise.all([
      lstatOrNull(this.#root),
      lstatOrNull(tombstonePath),
      lstatOrNull(disposalPath),
    ]);
    if (finalEntries.some((entry) => entry !== null)) {
      throw new Error(
        'Reset cannot publish completion while a live, tombstone, or disposal root exists',
      );
    }
    await this.#injectResetFault('before-journal-remove');
    await this.#durableRemove(this.#journalPath);
    this.#prepared = false;
    await this.#injectResetFault('after-journal-remove');
    return { recovered: true };
  }

  async prepareReset(): Promise<void> {
    if (!this.#canPrepareDestructiveReset) await failClosedIdentityBoundRemoval();
    const canonicalRoot = await this.#assertCanonicalOwnedLocation();
    const marker = await this.#readOwnershipMarker(this.#markerPath);
    const identity = rootIdentity(canonicalRoot);
    if (marker.rootIdentity !== identity) {
      throw new Error('Application data ownership could not be verified');
    }
    const metadata = await lstat(this.#root, { bigint: true });
    if (metadata.isSymbolicLink()) {
      throw new Error('Refusing a symbolic-link or junction application data root');
    }
    const nonce = randomUUID();
    const tombstonePath = resetTombstonePath(this.#root, identity, nonce);
    const disposalPath = resetDisposalPath(this.#root, identity, nonce);
    if ((await lstatOrNull(tombstonePath)) !== null || (await lstatOrNull(disposalPath)) !== null) {
      throw new Error('Reset tombstone or disposal directory already exists');
    }
    await this.#writeResetJournal(this.#journalPath, {
      schemaVersion: RESET_JOURNAL_VERSION,
      appId: APP_OWNERSHIP_ID,
      userDataRoot: this.#root,
      rootIdentity: identity,
      rootFileIdentity: fileIdentity(metadata),
      tombstonePath,
      disposalPath,
      phase: 'rename-pending',
      requestedAt: Date.now(),
      nonce,
    });
    this.#prepared = true;
    await this.#injectResetFault('after-journal-write');
  }

  async cancelPreparedReset(): Promise<void> {
    const journal = await this.#readJournal();
    if (journal !== null) {
      const { tombstonePath, disposalPath } = this.#validateJournalBinding(journal);
      if (
        (await lstatOrNull(tombstonePath)) !== null ||
        (await lstatOrNull(disposalPath)) !== null
      ) {
        throw new Error('Cannot cancel reset after the live root was renamed');
      }
    }
    await this.#durableRemove(this.#journalPath);
    if (journal !== null) {
      const { tombstonePath, disposalPath } = this.#validateJournalBinding(journal);
      if (
        (await lstatOrNull(tombstonePath)) !== null ||
        (await lstatOrNull(disposalPath)) !== null
      ) {
        await this.#writeResetJournal(this.#journalPath, journal);
        throw new Error(
          'Reset moved after cancellation validation; recovery authority was restored',
        );
      }
    }
    this.#prepared = false;
  }

  get resetPrepared(): boolean {
    return this.#prepared;
  }

  #validateJournalBinding(journal: z.infer<typeof ResetJournalSchema>): {
    tombstonePath: string;
    disposalPath: string;
  } {
    if (resolve(journal.userDataRoot) !== this.#root) {
      throw new Error('Reset journal does not match the application data root');
    }
    const tombstonePath = resetTombstonePath(this.#root, journal.rootIdentity, journal.nonce);
    const disposalPath = resetDisposalPath(this.#root, journal.rootIdentity, journal.nonce);
    if (
      resolve(journal.tombstonePath) !== tombstonePath ||
      resolve(journal.disposalPath) !== disposalPath ||
      dirname(tombstonePath) !== dirname(this.#root) ||
      dirname(disposalPath) !== dirname(this.#root) ||
      tombstonePath === disposalPath
    ) {
      throw new Error('Reset journal tombstone or disposal binding is invalid');
    }
    return { tombstonePath, disposalPath };
  }

  async #assertCanonicalOwnedLocation(): Promise<string> {
    const rootMetadata = await lstat(this.#root);
    if (rootMetadata.isSymbolicLink()) {
      throw new Error('Refusing a symbolic-link or junction application data root');
    }
    const [canonicalBase, canonicalRoot, canonicalHome] = await Promise.all([
      realpath(this.#allowedBase),
      realpath(this.#root),
      realpath(this.#home).catch(() => this.#home),
    ]);
    assertCanonicallyContained(canonicalBase, canonicalRoot);
    assertNotHomeOrProfileAncestor(canonicalRoot, canonicalHome);
    return canonicalRoot;
  }

  async #restoreUnverifiedRename(
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

  async #assertSafeResetDirectory(
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

  async #durableRename(source: string, destination: string): Promise<void> {
    await rename(source, destination);
    await this.#syncResetDirectory(dirname(source));
    if (dirname(destination) !== dirname(source)) {
      await this.#syncResetDirectory(dirname(destination));
    }
  }

  async #durableRemove(path: string): Promise<void> {
    await rm(path, { force: true });
    await this.#syncResetDirectory(dirname(path));
  }

  async #readOwnershipMarker(path: string): Promise<z.infer<typeof OwnershipMarkerSchema>> {
    const source = await readFile(path, 'utf8');
    return OwnershipMarkerSchema.parse(JSON.parse(source) as unknown);
  }

  async #readJournal(): Promise<z.infer<typeof ResetJournalSchema> | null> {
    let source: string;
    try {
      source = await readFile(this.#journalPath, 'utf8');
    } catch (error: unknown) {
      if (isNodeError(error) && error.code === 'ENOENT') return null;
      throw error;
    }
    const value = JSON.parse(source) as unknown;
    const legacyV2 = LegacyV2ResetJournalSchema.safeParse(value);
    if (legacyV2.success) {
      if (resolve(legacyV2.data.userDataRoot) !== this.#root) {
        throw new Error('Legacy reset journal does not match the application data root');
      }
      // Version 2 authorized recursive deletion of the live root. Cancel it rather than migrate
      // destructive authority into the tombstone protocol.
      await this.#durableRemove(this.#journalPath);
      this.#prepared = false;
      return null;
    }
    const legacyV3 = LegacyV3ResetJournalSchema.safeParse(value);
    if (legacyV3.success) {
      if (resolve(legacyV3.data.userDataRoot) !== this.#root) {
        throw new Error('Legacy reset journal does not match the application data root');
      }
      const disposalPath = resetDisposalPath(
        this.#root,
        legacyV3.data.rootIdentity,
        legacyV3.data.nonce,
      );
      if ((await lstatOrNull(disposalPath)) !== null) {
        throw new Error('Cannot migrate reset journal while its disposal path is occupied');
      }
      const migrated = {
        ...legacyV3.data,
        schemaVersion: RESET_JOURNAL_VERSION,
        disposalPath,
        phase: 'rename-pending' as const,
      };
      await this.#writeResetJournal(this.#journalPath, migrated);
      return ResetJournalSchema.parse(migrated);
    }
    return ResetJournalSchema.parse(value);
  }
}

export async function resetOwnedApplicationData(
  userDataRoot: string,
  options: DataLifecycleOptions,
): Promise<boolean> {
  const service = new DataLifecycleService(userDataRoot, options);
  const recovery = await service.recoverPendingReset();
  if (recovery.recovered) return true;
  const exists = await lstat(userDataRoot).then(
    () => true,
    (error: unknown) => {
      if (isNodeError(error) && error.code === 'ENOENT') return false;
      throw error;
    },
  );
  if (!exists) return false;
  await service.prepareReset();
  await service.recoverPendingReset();
  return true;
}

export function resetJournalPath(userDataRoot: string): string {
  const root = validateUserDataRoot(userDataRoot);
  const identity = createHash('sha256').update(root).digest('hex').slice(0, 24);
  return resolve(dirname(root), `.talking-quill-reset-${identity}.json`);
}

export function resetTombstonePath(userDataRoot: string, identity: string, nonce: string): string {
  if (!/^[a-f0-9]{64}$/u.test(identity) || !z.uuid().safeParse(nonce).success) {
    throw new Error('Reset tombstone identity is invalid');
  }
  const root = validateUserDataRoot(userDataRoot);
  return resolve(
    dirname(root),
    `.talking-quill-reset-tombstone-${identity.slice(0, 24)}-${nonce.toLowerCase()}`,
  );
}

export function resetDisposalPath(userDataRoot: string, identity: string, nonce: string): string {
  if (!/^[a-f0-9]{64}$/u.test(identity) || !z.uuid().safeParse(nonce).success) {
    throw new Error('Reset disposal identity is invalid');
  }
  const root = validateUserDataRoot(userDataRoot);
  return resolve(
    dirname(root),
    `.talking-quill-reset-disposal-${identity.slice(0, 24)}-${nonce.toLowerCase()}`,
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

function ownershipMarker(canonicalRoot: string): z.infer<typeof OwnershipMarkerSchema> {
  return {
    schemaVersion: OWNERSHIP_MARKER_VERSION,
    appId: APP_OWNERSHIP_ID,
    rootIdentity: rootIdentity(canonicalRoot),
  };
}

function rootIdentity(canonicalRoot: string): string {
  return createHash('sha256').update(`${APP_OWNERSHIP_ID}\0${canonicalRoot}`).digest('hex');
}

function fileIdentity(metadata: BigIntStats): string {
  return `${String(metadata.dev)}:${String(metadata.ino)}`;
}

async function lstatOrNull(path: string): Promise<BigIntStats | null> {
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

function failClosedIdentityBoundRemoval(): Promise<void> {
  return Promise.reject(
    new Error('Identity-bound recursive reset deletion is unavailable on this build'),
  );
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
