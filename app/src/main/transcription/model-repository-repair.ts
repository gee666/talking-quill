import type { link } from 'node:fs/promises';
import { rm } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import type {
  ModelManifestEntry,
  VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import type { inspectFile } from './model-integrity';
import {
  canDownloadInsteadOfLink,
  ensureSafeDirectory,
  fileSystemIdentityKey,
  verifiedIdentityStillCurrent,
} from './model-repository-filesystem';

/** Reuses verified installed files without trusting leftover staging hard links. */
export async function reuseInstalledFiles(
  model: ModelManifestEntry,
  installedDirectory: string,
  stagingDirectory: string,
  temporaryRoot: string,
  inspect: typeof inspectFile,
  linkFile: typeof link,
  identities: readonly VerifiedModelFileIdentity[],
  signal: AbortSignal,
): Promise<boolean> {
  let stagingContainsInstalledHardLinks = false;
  const installedFileKeys = new Set(identities.map(fileSystemIdentityKey));
  for (const identity of identities) {
    signal.throwIfAborted();
    const file = model.files.find((candidate) => candidate.path === identity.path);
    if (file === undefined) continue;
    const staged = join(stagingDirectory, ...file.path.split('/'));
    const stagedInspection = await inspect(staged, file.size, file.sha256, true, signal);
    if (stagedInspection.valid) {
      if (
        stagedInspection.identity !== null &&
        installedFileKeys.has(fileSystemIdentityKey(stagedInspection.identity))
      ) {
        stagingContainsInstalledHardLinks = true;
      }
      continue;
    }
    const installed = join(installedDirectory, ...file.path.split('/'));
    if (!(await verifiedIdentityStillCurrent(installed, identity))) continue;
    await ensureSafeDirectory(temporaryRoot, dirname(staged));
    await Promise.all([rm(staged, { force: true }), rm(`${staged}.part`, { force: true })]);
    try {
      await linkFile(installed, staged);
      stagingContainsInstalledHardLinks = true;
    } catch (error: unknown) {
      if (!canDownloadInsteadOfLink(error)) throw error;
      // The normal download path must include this file in free-space and SHA-256 checks.
      await rm(staged, { force: true });
    }
  }
  return stagingContainsInstalledHardLinks;
}
