import { createHash } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import { join, relative, resolve } from 'node:path';
import type { ModelManifestEntry } from '../../shared/schemas/model-manifest';
import type { WhisperWorkerErrorCode } from '../../shared/schemas/whisper-protocol';

export class WorkerModelVerificationError extends Error {
  readonly code: Extract<WhisperWorkerErrorCode, 'MODEL_MISSING' | 'MODEL_CORRUPT'>;

  constructor(code: Extract<WhisperWorkerErrorCode, 'MODEL_MISSING' | 'MODEL_CORRUPT'>) {
    super(
      code === 'MODEL_MISSING'
        ? 'Local model files are missing.'
        : 'Local model files are corrupt.',
    );
    this.name = 'WorkerModelVerificationError';
    this.code = code;
  }
}

export async function verifyModelFiles(
  cacheDirectory: string,
  model: ModelManifestEntry,
): Promise<void> {
  const root = await verifyModelRoot(cacheDirectory, model);
  for (const file of model.files) {
    const path = join(root, ...file.path.split('/'));
    const metadata = await safeModelLstat(path);
    if (metadata.size !== file.size || (await secureSha256(path, metadata)) !== file.sha256) {
      throw new WorkerModelVerificationError('MODEL_CORRUPT');
    }
  }
}

async function verifyModelRoot(cacheDirectory: string, model: ModelManifestEntry): Promise<string> {
  const cacheRoot = resolve(cacheDirectory);
  const root = resolve(cacheRoot, ...model.id.split('/'), model.revision);
  if (relative(cacheRoot, root).startsWith('..')) {
    throw new WorkerModelVerificationError('MODEL_CORRUPT');
  }
  let current = cacheRoot;
  let cacheMetadata: Stats;
  try {
    cacheMetadata = await lstat(current);
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) throw new WorkerModelVerificationError('MODEL_MISSING');
    throw error;
  }
  if (!cacheMetadata.isDirectory() || cacheMetadata.isSymbolicLink()) {
    throw new WorkerModelVerificationError('MODEL_CORRUPT');
  }
  for (const segment of relative(cacheRoot, root).split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    let metadata: Stats;
    try {
      metadata = await lstat(current);
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) throw new WorkerModelVerificationError('MODEL_MISSING');
      throw error;
    }
    if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
      throw new WorkerModelVerificationError('MODEL_CORRUPT');
    }
  }
  return root;
}

async function safeModelLstat(path: string): Promise<Stats> {
  let metadata: Stats;
  try {
    metadata = await lstat(path);
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) throw new WorkerModelVerificationError('MODEL_MISSING');
    throw error;
  }
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new WorkerModelVerificationError('MODEL_CORRUPT');
  }
  return metadata;
}

async function secureSha256(path: string, before: Stats): Promise<string> {
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat();
    if (!opened.isFile() || !sameStats(before, opened)) {
      throw new WorkerModelVerificationError('MODEL_CORRUPT');
    }
    const digest = createHash('sha256');
    for await (const chunk of handle.createReadStream({ autoClose: false, start: 0 })) {
      const value: unknown = chunk;
      if (!Buffer.isBuffer(value)) throw new WorkerModelVerificationError('MODEL_CORRUPT');
      digest.update(value);
    }
    const [afterHandle, afterPath] = await Promise.all([handle.stat(), safeModelLstat(path)]);
    if (!sameStats(opened, afterHandle) || !sameStats(afterHandle, afterPath)) {
      throw new WorkerModelVerificationError('MODEL_CORRUPT');
    }
    return digest.digest('hex');
  } finally {
    await handle.close();
  }
}

function sameStats(first: Stats, second: Stats): boolean {
  return (
    first.size === second.size &&
    first.mtimeMs === second.mtimeMs &&
    first.ctimeMs === second.ctimeMs &&
    first.birthtimeMs === second.birthtimeMs &&
    first.dev === second.dev &&
    first.ino === second.ino
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
