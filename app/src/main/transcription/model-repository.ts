import { randomUUID } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import {
  link,
  lstat,
  mkdir,
  open,
  readdir,
  rename,
  rm,
  statfs,
  writeFile,
  type FileHandle,
} from 'node:fs/promises';
import { dirname, join, relative, resolve } from 'node:path';
import {
  MODEL_DOWNLOAD_HEADROOM_RATIO,
  MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
} from '../../shared/constants/whisper';
import {
  VerifiedModelFileIdentitySchema,
  type ModelManifestEntry,
  type ModelManifestFile,
  type VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import { ModelManagerError } from './errors';
import { inspectFile, sameVerifiedIdentity, type FileIntegrity } from './model-integrity';
import {
  publishAtomically,
  publishRevisionDirectory,
  publishStagedFile,
  recoverRevisionDirectory,
  type RevisionBackupRemover,
} from './model-publication';

const COMPLETION_MARKER = '.talking-quill-complete.json';

interface ModelRepositoryOptions {
  readonly modelsDirectory: string;
  readonly temporaryDirectory: string;
  readonly availableBytes?: (path: string) => Promise<number>;
  readonly inspectFile?: typeof inspectFile;
  readonly rename?: typeof rename;
  readonly link?: typeof link;
  readonly removeRevisionBackup?: RevisionBackupRemover;
}

interface ModelInspection {
  readonly valid: boolean;
  readonly validBytes: number;
  readonly existingBytes: number;
  readonly corrupt: boolean;
  readonly identities: readonly VerifiedModelFileIdentity[];
}

interface CompletionMarker {
  readonly present: boolean;
  readonly identity: readonly VerifiedModelFileIdentity[] | null;
}

interface ModelPartialWriter {
  write(bytes: Uint8Array): Promise<void>;
  sync(): Promise<void>;
  close(): Promise<void>;
}

/** Owns the persisted model layout, secure staging, integrity metadata, and publication. */
export class ModelRepository {
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
    return this.#inspectModelDirectory(
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
    return this.#inspectModelDirectory(
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
    const headroom = Math.max(
      MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
      Math.ceil(remaining * MODEL_DOWNLOAD_HEADROOM_RATIO),
    );
    if (
      remaining > 0 &&
      (await this.#availableBytes(this.#temporaryDirectory)) < remaining + headroom
    ) {
      throw new ModelManagerError('DISK_SPACE', 'Not enough disk space to download this model.');
    }
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
    const path = join(this.#modelDirectory(model), COMPLETION_MARKER);
    let text: string;
    try {
      const before = await lstat(path);
      if (!before.isFile() || before.isSymbolicLink() || before.size > 64 * 1024) {
        return { present: true, identity: null };
      }
      const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const opened = await handle.stat();
        if (!opened.isFile() || !sameOpenedFile(before, opened)) {
          return { present: true, identity: null };
        }
        text = await handle.readFile('utf8');
        const [afterHandle, afterPath] = await Promise.all([handle.stat(), lstat(path)]);
        if (
          afterPath.isSymbolicLink() ||
          !afterPath.isFile() ||
          !sameOpenedFile(opened, afterHandle) ||
          !sameOpenedFile(afterHandle, afterPath)
        ) {
          return { present: true, identity: null };
        }
      } finally {
        await handle.close();
      }
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) return { present: false, identity: null };
      return { present: true, identity: null };
    }
    try {
      const value: unknown = JSON.parse(text);
      if (typeof value !== 'object' || value === null) return { present: true, identity: null };
      const record = value as Readonly<Record<string, unknown>>;
      const files = VerifiedModelFileIdentitySchema.array()
        .length(model.files.length)
        .safeParse(record.files);
      if (
        record.schemaVersion !== 1 ||
        record.revision !== model.revision ||
        record.totalBytes !== model.totalBytes ||
        !files.success ||
        files.data.some((file, index) => file.path !== model.files[index]?.path)
      ) {
        return { present: true, identity: null };
      }
      return { present: true, identity: files.data };
    } catch {
      return { present: true, identity: null };
    }
  }

  async commitVerification(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    if (identities.length !== model.files.length) {
      throw new ModelManagerError('CORRUPT', 'Verified model identity was incomplete.', true);
    }
    await this.#writeMarkerAt(
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
    return this.#identityStillCurrent(model, identities);
  }

  stagedIdentityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<boolean> {
    return this.#identityStillCurrent(
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
    let stagingContainsInstalledHardLinks = false;
    const installedFileKeys = new Set(identities.map(fileSystemIdentityKey));
    for (const identity of identities) {
      signal.throwIfAborted();
      const file = model.files.find((candidate) => candidate.path === identity.path);
      if (file === undefined) continue;
      const staged = this.#stagedTargetPath(model, file);
      const stagedInspection = await this.#inspectFile(
        staged,
        file.size,
        file.sha256,
        true,
        signal,
      );
      if (stagedInspection.valid) {
        if (
          stagedInspection.identity !== null &&
          installedFileKeys.has(fileSystemIdentityKey(stagedInspection.identity))
        ) {
          stagingContainsInstalledHardLinks = true;
        }
        continue;
      }
      const installed = join(this.#modelDirectory(model), ...file.path.split('/'));
      if (!(await verifiedIdentityStillCurrent(installed, identity))) continue;
      await ensureSafeDirectory(this.#temporaryDirectory, dirname(staged));
      await Promise.all([
        rm(staged, { force: true }),
        rm(this.#partPath(model, file), { force: true }),
      ]);
      try {
        await this.#link(installed, staged);
        stagingContainsInstalledHardLinks = true;
      } catch (error: unknown) {
        if (!canDownloadInsteadOfLink(error)) throw error;
        // The normal download path must include this file in free-space and SHA-256 checks.
        await rm(staged, { force: true });
      }
    }
    return stagingContainsInstalledHardLinks;
  }

  async prepareStagedPublication(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    await Promise.all(model.files.map((file) => rm(this.#partPath(model, file), { force: true })));
    await this.#assertOnlyManifestEntries(model);
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
    return this.#assertOnlyManifestEntries(model, this.#modelDirectory(model));
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
    await this.#removeObsoleteRevisions(dirname(target), model.revision);
    const temporaryTarget = this.#temporaryModelDirectory(model);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(temporaryTarget));
    await this.#removeObsoleteRevisions(dirname(temporaryTarget), model.revision);
  }

  async #inspectModelDirectory(
    model: ModelManifestEntry,
    directory: string,
    managedRoot: string,
    signal?: AbortSignal,
    hash = true,
  ): Promise<ModelInspection> {
    let validBytes = 0;
    let existingBytes = 0;
    let corrupt = false;
    const identities: VerifiedModelFileIdentity[] = [];
    try {
      for (const file of model.files) {
        const target = join(directory, ...file.path.split('/'));
        await assertSafeExistingDirectoryChain(managedRoot, dirname(target));
      }
    } catch (error: unknown) {
      if (error instanceof ModelManagerError && error.code === 'CORRUPT') {
        return {
          valid: false,
          validBytes: 0,
          existingBytes: 1,
          corrupt: true,
          identities,
        };
      }
      throw error;
    }
    for (const file of model.files) {
      signal?.throwIfAborted();
      const target = join(directory, ...file.path.split('/'));
      const result = await this.#inspectFile(target, file.size, file.sha256, hash, signal);
      if (result.exists) existingBytes += result.size;
      if (result.valid) {
        validBytes += file.size;
        if (result.identity !== null) identities.push({ path: file.path, ...result.identity });
      } else if (result.exists) corrupt = true;
    }
    return {
      valid: validBytes === model.totalBytes && identities.length === model.files.length,
      validBytes,
      existingBytes,
      corrupt,
      identities,
    };
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

  async #writeMarkerAt(
    model: ModelManifestEntry,
    directory: string,
    identities: readonly VerifiedModelFileIdentity[],
    managedRoot: string,
  ): Promise<void> {
    await ensureSafeDirectory(managedRoot, directory);
    const temporary = join(directory, `${COMPLETION_MARKER}.${randomUUID()}.tmp`);
    try {
      await writeFile(
        temporary,
        `${JSON.stringify({ schemaVersion: 1, revision: model.revision, totalBytes: model.totalBytes, files: identities })}\n`,
        { mode: 0o600, flag: 'wx' },
      );
      await publishAtomically(temporary, join(directory, COMPLETION_MARKER), this.#rename);
    } finally {
      await rm(temporary, { force: true }).catch(() => undefined);
    }
  }

  async #identityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
    directory = this.#modelDirectory(model),
    managedRoot = this.#modelsDirectory,
  ): Promise<boolean> {
    for (const expected of identities) {
      const file = model.files.find((candidate) => candidate.path === expected.path);
      if (file?.size !== expected.size) return false;
      const target = join(directory, ...file.path.split('/'));
      try {
        await assertSafeExistingDirectoryChain(managedRoot, dirname(target));
      } catch {
        return false;
      }
      try {
        const metadata = await lstat(target);
        if (
          !metadata.isFile() ||
          metadata.isSymbolicLink() ||
          !sameVerifiedIdentity(expected, metadata)
        ) {
          return false;
        }
      } catch {
        return false;
      }
    }
    return true;
  }

  async #assertOnlyManifestEntries(
    model: ModelManifestEntry,
    rootDirectory = this.#temporaryModelDirectory(model),
  ): Promise<void> {
    const expectedFiles = new Set(model.files.map((file) => file.path));
    const expectedDirectories = new Set<string>();
    for (const file of model.files) {
      const segments = file.path.split('/');
      for (let index = 1; index < segments.length; index += 1) {
        expectedDirectories.add(segments.slice(0, index).join('/'));
      }
    }

    const visit = async (directory: string, relativeDirectory: string): Promise<void> => {
      const entries = await readdir(directory, { withFileTypes: true });
      for (const entry of entries) {
        const relativePath =
          relativeDirectory === '' ? entry.name : `${relativeDirectory}/${entry.name}`;
        if (expectedFiles.has(relativePath)) {
          if (!entry.isFile() || entry.isSymbolicLink()) {
            throw new ModelManagerError(
              'CORRUPT',
              'Model staging contains an invalid manifest entry.',
              true,
            );
          }
          continue;
        }
        if (
          expectedDirectories.has(relativePath) &&
          entry.isDirectory() &&
          !entry.isSymbolicLink()
        ) {
          await visit(join(directory, entry.name), relativePath);
          continue;
        }
        throw new ModelManagerError(
          'CORRUPT',
          `Model staging contains unexpected entry: ${relativePath}.`,
          true,
        );
      }
    };

    await visit(rootDirectory, '');
  }

  async #removeObsoleteRevisions(parent: string, currentRevision: string): Promise<void> {
    const entries = await readdir(parent, { withFileTypes: true });
    await Promise.all(
      entries
        .filter(
          (entry) =>
            entry.name !== currentRevision &&
            /^[a-f0-9]{40}$/u.test(entry.name) &&
            entry.isDirectory() &&
            !entry.isSymbolicLink(),
        )
        .map((entry) => rm(join(parent, entry.name), { recursive: true, force: true })),
    );
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

class FileHandlePartialWriter implements ModelPartialWriter {
  readonly #handle: FileHandle;

  constructor(handle: FileHandle) {
    this.#handle = handle;
  }

  async write(bytes: Uint8Array): Promise<void> {
    let offset = 0;
    while (offset < bytes.byteLength) {
      const result = await this.#handle.write(bytes, offset, bytes.byteLength - offset);
      if (result.bytesWritten <= 0) {
        throw new ModelManagerError('IO', 'Unable to write model data.');
      }
      offset += result.bytesWritten;
    }
  }

  sync(): Promise<void> {
    return this.#handle.sync();
  }

  close(): Promise<void> {
    return this.#handle.close();
  }
}

async function assertSameFilesystem(first: string, second: string): Promise<void> {
  const [firstMetadata, secondMetadata] = await Promise.all([lstat(first), lstat(second)]);
  if (firstMetadata.dev !== secondMetadata.dev) {
    throw new ModelManagerError(
      'PROTOCOL',
      'Model staging must use the same filesystem as installed models.',
    );
  }
}

async function ensureSafeDirectory(root: string, destination: string): Promise<void> {
  const absoluteRoot = resolve(root);
  const absoluteDestination = resolve(destination);
  const suffix = relative(absoluteRoot, absoluteDestination);
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  await mkdir(absoluteRoot, { recursive: true, mode: 0o700 });
  let current = absoluteRoot;
  await assertDirectoryNotLink(current);
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await mkdir(current, { mode: 0o700 });
    } catch (error: unknown) {
      if (!hasCode(error, 'EEXIST')) throw error;
    }
    await assertDirectoryNotLink(current);
  }
}

async function assertDirectoryNotLink(path: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new ModelManagerError('CORRUPT', 'Managed model directory contains a link.', true);
  }
}

/** Validates existing parents without turning a read-only status check into a filesystem mutation. */
async function assertSafeExistingDirectoryChain(root: string, destination: string): Promise<void> {
  const absoluteRoot = resolve(root);
  const suffix = relative(absoluteRoot, resolve(destination));
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  let current = absoluteRoot;
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await assertDirectoryNotLink(current);
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) return;
      throw error;
    }
  }
}

async function safeRegularFileSize(path: string): Promise<number> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new ModelManagerError('CORRUPT', 'Managed model file is not a regular file.', true);
    }
    return metadata.size;
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return 0;
    throw error;
  }
}

async function verifiedIdentityStillCurrent(
  path: string,
  expected: Omit<VerifiedModelFileIdentity, 'path'>,
): Promise<boolean> {
  try {
    const metadata = await lstat(path);
    return (
      metadata.isFile() && !metadata.isSymbolicLink() && sameVerifiedIdentity(expected, metadata)
    );
  } catch {
    return false;
  }
}

async function openSafePart(path: string, offset: number): Promise<FileHandle> {
  const flags =
    constants.O_WRONLY |
    constants.O_CREAT |
    constants.O_NOFOLLOW |
    (offset === 0 ? constants.O_TRUNC : constants.O_APPEND);
  const handle = await open(path, flags, 0o600);
  const metadata = await handle.stat();
  let pathMetadata: Stats;
  try {
    pathMetadata = await lstat(path);
  } catch (error: unknown) {
    await handle.close();
    throw error;
  }
  if (
    !metadata.isFile() ||
    metadata.size !== offset ||
    pathMetadata.isSymbolicLink() ||
    !pathMetadata.isFile() ||
    !sameOpenedFile(metadata, pathMetadata)
  ) {
    await handle.close();
    throw new ModelManagerError(
      'CORRUPT',
      'Download staging path changed during secure open.',
      true,
    );
  }
  return handle;
}

function fileSystemIdentityKey(
  identity: Pick<VerifiedModelFileIdentity, 'device' | 'inode'>,
): string {
  return `${identity.device}:${identity.inode}`;
}

function sameOpenedFile(first: Stats, second: Stats): boolean {
  return (
    first.size === second.size &&
    first.mtimeMs === second.mtimeMs &&
    first.ctimeMs === second.ctimeMs &&
    first.birthtimeMs === second.birthtimeMs &&
    first.dev === second.dev &&
    first.ino === second.ino
  );
}

async function defaultAvailableBytes(path: string): Promise<number> {
  const values = await statfs(path);
  const available = BigInt(values.bavail) * BigInt(values.bsize);
  return available > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(available);
}

function canDownloadInsteadOfLink(error: unknown): boolean {
  return ['EXDEV', 'EPERM', 'EACCES', 'ENOSYS', 'ENOTSUP', 'EOPNOTSUPP', 'EMLINK', 'ENOENT'].some(
    (code) => hasCode(error, code),
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
