import { lstat } from 'node:fs/promises';
import { dirname } from 'node:path';
import type { DataLifecycleOptions, ResetRecoveryResult } from './data-lifecycle-service';
import type { ResetJournal } from './reset-journal';
import {
  type OwnedDataLocation,
  fileIdentity,
  lstatOrNull,
  rootIdentity,
} from './owned-data-location';

interface ResetRecoveryContext {
  readonly journal: ResetJournal | null;
  readonly journalPath: string;
  readonly markerPath: string;
  readonly location: OwnedDataLocation;
  readonly readOwnershipMarker: (path: string) => Promise<{ readonly rootIdentity: string }>;
  readonly writeResetJournal: NonNullable<DataLifecycleOptions['writeResetJournal']>;
  readonly injectResetFault: NonNullable<DataLifecycleOptions['injectResetFault']>;
  readonly removeIdentityBoundDirectory: NonNullable<
    DataLifecycleOptions['removeIdentityBoundDirectory']
  >;
  readonly syncResetDirectory: NonNullable<DataLifecycleOptions['syncResetDirectory']>;
  readonly durableRename: (source: string, destination: string) => Promise<void>;
  readonly durableRemove: (path: string) => Promise<void>;
  readonly onCompleted: () => void;
}

// Keep fault boundaries and durability operations in protocol order.
export async function recoverReset(context: ResetRecoveryContext): Promise<ResetRecoveryResult> {
  let journal = context.journal;
  if (journal === null) return { recovered: false };
  const { tombstonePath, disposalPath } = context.location.validateJournalBinding(journal);
  const [liveMetadata, tombstoneMetadata, disposalMetadata] = await Promise.all([
    lstatOrNull(context.location.root),
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
    const canonicalRoot = await context.location.assertCanonicalOwnedLocation();
    const marker = await context.readOwnershipMarker(context.markerPath);
    const expectedIdentity = rootIdentity(canonicalRoot);
    if (
      marker.rootIdentity !== expectedIdentity ||
      journal.rootIdentity !== expectedIdentity ||
      journal.rootFileIdentity !== fileIdentity(liveMetadata)
    ) {
      throw new Error('Reset journal ownership could not be verified');
    }
    await context.injectResetFault('before-live-rename');
    await context.durableRename(context.location.root, tombstonePath);
    await context.injectResetFault('before-renamed-identity-check');
    const renamedMetadata = await lstat(tombstonePath, { bigint: true });
    if (
      renamedMetadata.isSymbolicLink() ||
      fileIdentity(renamedMetadata) !== journal.rootFileIdentity
    ) {
      await context.location.restoreUnverifiedRename(
        tombstonePath,
        context.location.root,
        renamedMetadata,
      );
      throw new Error(
        'Application data root changed during atomic reset rename; reset remains quarantined',
      );
    }
    await context.injectResetFault('after-live-rename');
  } else if (tombstoneMetadata !== null) {
    await context.location.assertSafeResetDirectory(
      tombstonePath,
      tombstoneMetadata,
      journal.rootFileIdentity,
      'tombstone',
    );
  } else if (disposalMetadata !== null) {
    await context.location.assertSafeResetDirectory(
      disposalPath,
      disposalMetadata,
      journal.rootFileIdentity,
      'disposal',
    );
  }

  const currentTombstone = await lstatOrNull(tombstonePath);
  if (currentTombstone !== null) {
    await context.location.assertSafeResetDirectory(
      tombstonePath,
      currentTombstone,
      journal.rootFileIdentity,
      'tombstone',
    );
    await context.injectResetFault('before-tombstone-remove');
    const transitionMetadata = await lstatOrNull(tombstonePath);
    if (transitionMetadata === null) {
      throw new Error('Reset tombstone disappeared before disposal transition');
    }
    await context.location.assertSafeResetDirectory(
      tombstonePath,
      transitionMetadata,
      journal.rootFileIdentity,
      'tombstone',
    );
    if (journal.phase !== 'disposal-pending') {
      journal = { ...journal, phase: 'disposal-pending' };
      await context.writeResetJournal(context.journalPath, journal);
    }
    await context.injectResetFault('before-tombstone-disposal-transition');
    await context.durableRename(tombstonePath, disposalPath);
    await context.injectResetFault('after-tombstone-disposal-transition');
    const transitionedMetadata = await lstat(disposalPath, { bigint: true });
    if (
      transitionedMetadata.isSymbolicLink() ||
      !transitionedMetadata.isDirectory() ||
      fileIdentity(transitionedMetadata) !== journal.rootFileIdentity
    ) {
      await context.location.restoreUnverifiedRename(
        disposalPath,
        tombstonePath,
        transitionedMetadata,
      );
      throw new Error(
        'Reset tombstone changed during atomic disposal transition; reset remains quarantined',
      );
    }
  }

  const currentDisposal = await lstatOrNull(disposalPath);
  if (currentDisposal !== null) {
    await context.location.assertSafeResetDirectory(
      disposalPath,
      currentDisposal,
      journal.rootFileIdentity,
      'disposal',
    );
    await context.injectResetFault('before-disposal-remove');
    const deletionMetadata = await lstatOrNull(disposalPath);
    if (deletionMetadata === null) {
      throw new Error('Reset disposal directory disappeared before deletion');
    }
    await context.location.assertSafeResetDirectory(
      disposalPath,
      deletionMetadata,
      journal.rootFileIdentity,
      'disposal',
    );
    await context.injectResetFault('before-identity-bound-remove');
    await context.removeIdentityBoundDirectory({
      path: disposalPath,
      expectedFileIdentity: journal.rootFileIdentity,
    });
    if ((await lstatOrNull(disposalPath)) !== null) {
      throw new Error('Identity-bound reset boundary did not remove the recorded directory');
    }
    await context.syncResetDirectory(dirname(disposalPath));
    await context.injectResetFault('after-tombstone-remove');
  }
  const finalEntries = await Promise.all([
    lstatOrNull(context.location.root),
    lstatOrNull(tombstonePath),
    lstatOrNull(disposalPath),
  ]);
  if (finalEntries.some((entry) => entry !== null)) {
    throw new Error(
      'Reset cannot publish completion while a live, tombstone, or disposal root exists',
    );
  }
  await context.injectResetFault('before-journal-remove');
  await context.durableRemove(context.journalPath);
  context.onCompleted();
  await context.injectResetFault('after-journal-remove');
  return { recovered: true };
}
