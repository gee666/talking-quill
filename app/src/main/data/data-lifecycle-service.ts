import {
  OwnedDataLocation,
  OwnershipMarkerSchema,
  ownershipMarker,
  rootIdentity,
  fileIdentity,
  lstatOrNull,
  isNodeError,
  resetJournalPath,
  resetTombstonePath,
  resetDisposalPath,
} from './owned-data-location';
import { recoverReset } from './reset-recovery';
export { resetJournalPath, validateUserDataRoot } from './owned-data-location';
import { randomUUID } from 'node:crypto';
import { lstat, readFile, rename, rm } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import type { z } from 'zod';
import { syncDirectory, writeJsonAtomic } from '../persistence/atomic-json';
import {
  APP_OWNERSHIP_ID,
  decodeResetJournal,
  RESET_JOURNAL_VERSION,
  type ResetJournal,
} from './reset-journal';

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
  readonly #location: OwnedDataLocation;
  readonly #journalPath: string;
  readonly #markerPath: string;
  readonly #writeResetJournal: (path: string, value: unknown) => Promise<void>;
  readonly #injectResetFault: (phase: ResetFaultPhase) => Promise<void>;
  readonly #removeIdentityBoundDirectory: (request: IdentityBoundRemovalRequest) => Promise<void>;
  readonly #syncResetDirectory: (path: string) => Promise<void>;
  readonly #canPrepareDestructiveReset: boolean;
  #prepared = false;

  constructor(userDataRoot: string, options: DataLifecycleOptions) {
    this.#location = new OwnedDataLocation(
      userDataRoot,
      options.allowedBase,
      options.homeDirectory,
    );
    this.#root = this.#location.root;
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

  /**
   * Rebinds a valid application-data tree copied to a different profile path.
   * The marker and temporary session files are path-bound runtime data. User
   * settings, models, commands, history, and credentials remain untouched.
   */
  async reconcileCopiedProfile(): Promise<boolean> {
    const canonicalRoot = await this.#location.assertCanonicalOwnedLocation();
    // These directories are app-owned process state, never profile data. Clear
    // them on every cold start, including copies that keep the same Windows
    // user name and therefore the same lexical AppData path.
    await Promise.all(
      ['tmp', 'runtime'].map((name) =>
        rm(resolve(this.#root, name), { recursive: true, force: true }),
      ),
    );
    let source: string | null = null;
    try {
      source = await readFile(this.#markerPath, 'utf8');
    } catch (error: unknown) {
      if (!isNodeError(error) || error.code !== 'ENOENT') throw error;
    }
    if (source === null) return false;
    const marker = OwnershipMarkerSchema.parse(JSON.parse(source) as unknown);
    const expected = ownershipMarker(canonicalRoot);
    if (marker.rootIdentity === expected.rootIdentity) return false;

    const resetJournal = await readFile(this.#journalPath, 'utf8').catch((error: unknown) => {
      if (isNodeError(error) && error.code === 'ENOENT') return null;
      throw error;
    });
    if (resetJournal !== null) {
      throw new Error('A copied application profile contains pending reset state');
    }
    await writeJsonAtomic(this.#markerPath, expected);
    return true;
  }

  async initializeOwnership(): Promise<void> {
    const canonicalRoot = await this.#location.assertCanonicalOwnedLocation();
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
    return recoverReset({
      journal: await this.#readJournal(),
      journalPath: this.#journalPath,
      markerPath: this.#markerPath,
      location: this.#location,
      readOwnershipMarker: (path) => this.#readOwnershipMarker(path),
      writeResetJournal: (path, value) => this.#writeResetJournal(path, value),
      injectResetFault: (phase) => this.#injectResetFault(phase),
      removeIdentityBoundDirectory: (request) => this.#removeIdentityBoundDirectory(request),
      syncResetDirectory: (path) => this.#syncResetDirectory(path),
      durableRename: (source, destination) => this.#durableRename(source, destination),
      durableRemove: (path) => this.#durableRemove(path),
      onCompleted: () => {
        this.#prepared = false;
      },
    });
  }

  async prepareReset(): Promise<void> {
    if (!this.#canPrepareDestructiveReset) await failClosedIdentityBoundRemoval();
    const canonicalRoot = await this.#location.assertCanonicalOwnedLocation();
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
      const { tombstonePath, disposalPath } = this.#location.validateJournalBinding(journal);
      if (
        (await lstatOrNull(tombstonePath)) !== null ||
        (await lstatOrNull(disposalPath)) !== null
      ) {
        throw new Error('Cannot cancel reset after the live root was renamed');
      }
    }
    await this.#durableRemove(this.#journalPath);
    if (journal !== null) {
      const { tombstonePath, disposalPath } = this.#location.validateJournalBinding(journal);
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

  async #readJournal(): Promise<ResetJournal | null> {
    let source: string;
    try {
      source = await readFile(this.#journalPath, 'utf8');
    } catch (error: unknown) {
      if (isNodeError(error) && error.code === 'ENOENT') return null;
      throw error;
    }
    const decoded = decodeResetJournal(JSON.parse(source) as unknown);
    if (decoded.kind === 'legacy-v2') {
      if (resolve(decoded.value.userDataRoot) !== this.#root) {
        throw new Error('Legacy reset journal does not match the application data root');
      }
      // Version 2 authorized recursive deletion of the live root. Cancel it rather than migrate
      // destructive authority into the tombstone protocol.
      await this.#durableRemove(this.#journalPath);
      this.#prepared = false;
      return null;
    }
    if (decoded.kind === 'legacy-v3') {
      if (resolve(decoded.value.userDataRoot) !== this.#root) {
        throw new Error('Legacy reset journal does not match the application data root');
      }
      const disposalPath = resetDisposalPath(
        this.#root,
        decoded.value.rootIdentity,
        decoded.value.nonce,
      );
      if ((await lstatOrNull(disposalPath)) !== null) {
        throw new Error('Cannot migrate reset journal while its disposal path is occupied');
      }
      const migrated: ResetJournal = {
        ...decoded.value,
        schemaVersion: RESET_JOURNAL_VERSION,
        disposalPath,
        phase: 'rename-pending',
      };
      await this.#writeResetJournal(this.#journalPath, migrated);
      return migrated;
    }
    return decoded.value;
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

function failClosedIdentityBoundRemoval(): Promise<void> {
  return Promise.reject(
    new Error('Identity-bound recursive reset deletion is unavailable on this build'),
  );
}
