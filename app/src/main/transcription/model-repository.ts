import { link, rename, rm } from 'node:fs/promises';
import { reuseInstalledFiles } from './model-repository-repair';
import { dirname, join, resolve } from 'node:path';
import type {
  ModelManifestEntry,
  ModelManifestFile,
  VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import { ModelManagerError } from './errors';
import { inspectFile, type FileIntegrity } from './model-integrity';
import {
  publishRevisionDirectory,
  publishStagedFile,
  recoverRevisionDirectory,
  type RevisionBackupRemover,
} from './model-publication';
import {
  assertSameFilesystem,
  defaultAvailableBytes,
  assertDownloadCapacity,
  ensureSafeDirectory,
  FileHandlePartialWriter,
  type ModelPartialWriter,
  openSafePart,
  removeObsoleteRevisions,
  safeRegularFileSize,
  verifiedIdentityStillCurrent,
} from './model-repository-filesystem';
import {
  COMPLETION_MARKER,
  ModelRepositoryMetadata,
  type CompletionMarker,
  type ModelInspection,
} from './model-repository-metadata';

interface ModelRepositoryOptions {
  readonly modelsDirectory: string;
  readonly temporaryDirectory: string;
  readonly availableBytes?: (path: string) => Promise<number>;
  readonly inspectFile?: typeof inspectFile;
  readonly rename?: typeof rename;
  readonly link?: typeof link;
  readonly removeRevisionBackup?: RevisionBackupRemover;
}

/** Owns the persisted model layout, secure staging, integrity metadata, and publication. */
export class ModelRepository {
  readonly #metadata: ModelRepositoryMetadata;
  readonly #modelsDirectory: string;
  readonly #temporaryDirectory: string;
  readonly #availableBytes: (path: string) => Promise<number>;
  readonly #inspectFile: typeof inspectFile;
  readonly #rename: typeof rename;
  readonly #link: typeof link;
  readonly #removeRevisionBackup: RevisionBackupRemover;

  constructor(options: ModelRepositoryOptions) {
    this.#modelsDirectory = resolve(options.modelsDirectory);
    this.#temporaryDirectory = resolve(options.temporaryDirectory);
    this.#availableBytes = options.availableBytes ?? defaultAvailableBytes;
    this.#inspectFile = options.inspectFile ?? inspectFile;
    this.#rename = options.rename ?? rename;
    this.#link = options.link ?? link;
    this.#metadata = new ModelRepositoryMetadata(this.#inspectFile, this.#rename);
    this.#removeRevisionBackup =
      options.removeRevisionBackup ?? ((path) => rm(path, { recursive: true, force: true }));
  }

  async prepareRoots(): Promise<void> {
    await ensureSafeDirectory(this.#modelsDirectory, this.#modelsDirectory);
    await ensureSafeDirectory(this.#temporaryDirectory, this.#temporaryDirectory);
    await assertSameFilesystem(this.#modelsDirectory, this.#temporaryDirectory);
  }

  prepareStaging(model: ModelManifestEntry): Promise<void> {
    return ensureSafeDirectory(this.#temporaryDirectory, this.#temporaryModelDirectory(model));
  }

  inspectInstalled(
    model: ModelManifestEntry,
    signal?: AbortSignal,
    hash = true,
  ): Promise<ModelInspection> {
    return this.#metadata.inspectModelDirectory(
      model,
      this.#modelDirectory(model),
      this.#modelsDirectory,
      signal,
      hash,
    );
  }

  inspectStaging(
    model: ModelManifestEntry,
    signal?: AbortSignal,
    hash = true,
  ): Promise<ModelInspection> {
    return this.#metadata.inspectModelDirectory(
      model,
      this.#temporaryModelDirectory(model),
      this.#temporaryDirectory,
      signal,
      hash,
    );
  }

  inspectStagedFile(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    signal?: AbortSignal,
    hash = true,
  ): Promise<FileIntegrity> {
    return this.#inspectFile(
      this.#stagedTargetPath(model, file),
      file.size,
      file.sha256,
      hash,
      signal,
    );
  }

  inspectPartial(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    signal?: AbortSignal,
  ): Promise<FileIntegrity> {
    return this.#inspectFile(this.#partPath(model, file), file.size, file.sha256, true, signal);
  }

  preparePartial(model: ModelManifestEntry, file: ModelManifestFile): Promise<void> {
    return ensureSafeDirectory(this.#temporaryDirectory, dirname(this.#partPath(model, file)));
  }

  partialSize(model: ModelManifestEntry, file: ModelManifestFile): Promise<number> {
    return safeRegularFileSize(this.#partPath(model, file));
  }

  removePartial(model: ModelManifestEntry, file: ModelManifestFile): Promise<void> {
    return rm(this.#partPath(model, file), { force: true });
  }

  removeStagedFile(model: ModelManifestEntry, file: ModelManifestFile): Promise<void> {
    return rm(this.#stagedTargetPath(model, file), { force: true });
  }

  async openPartialWriter(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    offset: number,
  ): Promise<ModelPartialWriter> {
    const part = this.#partPath(model, file);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(part));
    return new FileHandlePartialWriter(await openSafePart(part, offset));
  }

  async publishVerifiedPartial(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    verifiedIdentity: Omit<VerifiedModelFileIdentity, 'path'> | null,
    signal: AbortSignal,
  ): Promise<void> {
    const part = this.#partPath(model, file);
    let identity = verifiedIdentity;
    if (identity === null) {
      const inspection = await this.#inspectFile(part, file.size, file.sha256, true, signal);
      if (!inspection.valid || inspection.identity === null) {
        await rm(part, { force: true });
        throw new ModelManagerError('CORRUPT', `Checksum failed for ${file.path}.`, true);
      }
      identity = inspection.identity;
    }
    if (!(await verifiedIdentityStillCurrent(part, identity))) {
      await rm(part, { force: true });
      throw new ModelManagerError('CORRUPT', `Verified file changed for ${file.path}.`, true);
    }
    const target = this.#stagedTargetPath(model, file);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(target));
    await publishStagedFile(part, target, this.#rename);
  }

  async ensureDownloadCapacity(
    model: ModelManifestEntry,
    stagedIdentities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    const reusableBytes = await this.#reusableTemporaryBytes(model, stagedIdentities);
    const remaining = Math.max(0, model.totalBytes - reusableBytes);
    await assertDownloadCapacity(remaining, () => this.#availableBytes(this.#temporaryDirectory));
  }

  async temporaryBytes(model: ModelManifestEntry): Promise<number> {
    let total = 0;
    for (const file of model.files) {
      const staged = await safeRegularFileSize(this.#stagedTargetPath(model, file));
      if (staged === file.size) total += file.size;
      else total += Math.min(await safeRegularFileSize(this.#partPath(model, file)), file.size);
    }
    return total;
  }

  async readCompletionMarker(model: ModelManifestEntry): Promise<CompletionMarker> {
    return this.#metadata.readCompletionMarker(model, this.#modelDirectory(model));
  }

  async commitVerification(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    if (identities.length !== model.files.length) {
      throw new ModelManagerError('CORRUPT', 'Verified model identity was incomplete.', true);
    }
    await this.#metadata.writeMarkerAt(
      model,
      this.#modelDirectory(model),
      identities,
      this.#modelsDirectory,
    );
  }

  removeCompletionMarker(model: ModelManifestEntry): Promise<void> {
    return rm(join(this.#modelDirectory(model), COMPLETION_MARKER), { force: true });
  }

  installedIdentityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<boolean> {
    return this.#metadata.identityStillCurrent(
      model,
      identities,
      this.#modelDirectory(model),
      this.#modelsDirectory,
    );
  }

  stagedIdentityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<boolean> {
    return this.#metadata.identityStillCurrent(
      model,
      identities,
      this.#temporaryModelDirectory(model),
      this.#temporaryDirectory,
    );
  }

  async reuseInstalledFiles(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
    signal: AbortSignal,
  ): Promise<boolean> {
    return reuseInstalledFiles(
      model,
      this.#modelDirectory(model),
      this.#temporaryModelDirectory(model),
      this.#temporaryDirectory,
      this.#inspectFile,
      this.#link,
      identities,
      signal,
    );
  }

  async prepareStagedPublication(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    await Promise.all(model.files.map((file) => rm(this.#partPath(model, file), { force: true })));
    await this.#metadata.assertOnlyManifestEntries(model, this.#temporaryModelDirectory(model));
    if (!(await this.stagedIdentityStillCurrent(model, identities))) {
      throw new ModelManagerError('CORRUPT', 'Verified staging changed before publication.', true);
    }
  }

  publishStagedRevision(model: ModelManifestEntry): Promise<void> {
    return publishRevisionDirectory(
      this.#temporaryModelDirectory(model),
      this.#modelDirectory(model),
      this.#rename,
      this.#removeRevisionBackup,
    );
  }

  assertPublishedManifestEntries(model: ModelManifestEntry): Promise<void> {
    return this.#metadata.assertOnlyManifestEntries(model, this.#modelDirectory(model));
  }

  removeInstalledRevision(model: ModelManifestEntry): Promise<void> {
    return rm(this.#modelDirectory(model), { recursive: true, force: true });
  }

  async deleteArtifacts(model: ModelManifestEntry): Promise<void> {
    await Promise.all([
      rm(this.#modelDirectory(model), { recursive: true, force: true }),
      rm(this.#temporaryModelDirectory(model), { recursive: true, force: true }),
    ]);
  }

  removeTemporaryRevision(model: ModelManifestEntry): Promise<void> {
    return rm(this.#temporaryModelDirectory(model), { recursive: true, force: true });
  }

  async recoverArtifacts(model: ModelManifestEntry): Promise<void> {
    const target = this.#modelDirectory(model);
    await ensureSafeDirectory(this.#modelsDirectory, dirname(target));
    // Recovery intentionally uses the native rename operation, matching the prior persisted-layout
    // behavior; the injected rename seam is scoped to new publication attempts.
    await recoverRevisionDirectory(target, rename, this.#removeRevisionBackup);
    await removeObsoleteRevisions(dirname(target), model.revision);
    const temporaryTarget = this.#temporaryModelDirectory(model);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(temporaryTarget));
    await removeObsoleteRevisions(dirname(temporaryTarget), model.revision);
  }

  async #reusableTemporaryBytes(
    model: ModelManifestEntry,
    stagedIdentities: readonly VerifiedModelFileIdentity[],
  ): Promise<number> {
    const validStagedPaths = new Set(stagedIdentities.map((identity) => identity.path));
    let total = 0;
    for (const file of model.files) {
      total += validStagedPaths.has(file.path)
        ? file.size
        : Math.min(await safeRegularFileSize(this.#partPath(model, file)), file.size);
    }
    return total;
  }

  #stagedTargetPath(model: ModelManifestEntry, file: ModelManifestFile): string {
    return join(this.#temporaryModelDirectory(model), ...file.path.split('/'));
  }

  #partPath(model: ModelManifestEntry, file: ModelManifestFile): string {
    return `${this.#stagedTargetPath(model, file)}.part`;
  }

  #modelDirectory(model: ModelManifestEntry): string {
    return join(this.#modelsDirectory, ...model.id.split('/'), model.revision);
  }

  #temporaryModelDirectory(model: ModelManifestEntry): string {
    return join(this.#temporaryDirectory, ...model.id.split('/'), model.revision);
  }
}
