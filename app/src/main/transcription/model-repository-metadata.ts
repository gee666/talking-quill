import { randomUUID } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, open, readdir, type rename, rm, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import {
  VerifiedModelFileIdentitySchema,
  type ModelManifestEntry,
  type VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import { ModelManagerError } from './errors';
import { type inspectFile, sameVerifiedIdentity } from './model-integrity';
import { publishAtomically } from './model-publication';
import {
  assertSafeExistingDirectoryChain,
  ensureSafeDirectory,
  hasCode,
  sameOpenedFile,
} from './model-repository-filesystem';

export const COMPLETION_MARKER = '.talking-quill-complete.json';

export interface ModelInspection {
  readonly valid: boolean;
  readonly validBytes: number;
  readonly existingBytes: number;
  readonly corrupt: boolean;
  readonly identities: readonly VerifiedModelFileIdentity[];
}

export interface CompletionMarker {
  readonly present: boolean;
  readonly identity: readonly VerifiedModelFileIdentity[] | null;
}

/** Reads and validates persisted identities and manifest directory contents. */
export class ModelRepositoryMetadata {
  readonly #inspectFile: typeof inspectFile;
  readonly #rename: typeof rename;

  constructor(inspect: typeof inspectFile, renameFile: typeof rename) {
    this.#inspectFile = inspect;
    this.#rename = renameFile;
  }

  async readCompletionMarker(
    model: ModelManifestEntry,
    directory: string,
  ): Promise<CompletionMarker> {
    const path = join(directory, COMPLETION_MARKER);
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

  async inspectModelDirectory(
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

  async writeMarkerAt(
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

  async identityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
    directory: string,
    managedRoot: string,
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

  async assertOnlyManifestEntries(model: ModelManifestEntry, rootDirectory: string): Promise<void> {
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
}
