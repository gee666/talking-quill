import { extractFile, getRawHeader, statFile } from '@electron/asar';
import { lstatSync, statSync } from 'node:fs';
import { resolve, sep } from 'node:path';

const ONNX_NATIVE_FILENAMES = Object.freeze({
  darwin: new Set(['libonnxruntime.1.21.0.dylib', 'onnxruntime_binding.node']),
  linux: new Set([
    'libonnxruntime.so.1',
    'libonnxruntime.so.1.21.0',
    'libonnxruntime_providers_shared.so',
    'onnxruntime_binding.node',
  ]),
  win32: new Set(['DirectML.dll', 'onnxruntime.dll', 'onnxruntime_binding.node']),
});
const TARGET_ASAR_PLATFORM = Object.freeze({ mac: 'darwin', win: 'win32' });

export function* extractRegularAsarFiles(archivePath, entries, label = 'ASAR', options = {}) {
  const archiveSize = BigInt(statSync(archivePath).size);
  const dataOffset = BigInt(8 + getRawHeader(archivePath).headerSize);
  for (const entry of entries) {
    if (
      typeof entry !== 'string' ||
      entry.length === 0 ||
      entry.includes('\\') ||
      entry.split('/').some((part) => part.length === 0 || part === '.' || part === '..')
    ) {
      throw new Error(`${label} contains an invalid entry path`);
    }
    const archiveEntry = entry.replaceAll('/', sep);
    let metadata;
    try {
      metadata = statFile(archivePath, archiveEntry, false);
    } catch (error) {
      throw new Error(`${label} entry metadata cannot be read: ${entry}`, { cause: error });
    }
    const kind = asarEntryKind(metadata);
    if (kind === 'directory') continue;
    if (kind === 'link') throw new Error(`${label} link is not allowed: ${entry}`);
    if (kind !== 'file') throw new Error(`${label} entry is malformed: ${entry}`);
    if (metadata.unpacked === true) {
      let physicalMetadata;
      try {
        physicalMetadata = lstatSync(resolve(`${archivePath}.unpacked`, archiveEntry));
      } catch (error) {
        if (
          isAllowedPrunedOnnxNative(
            entry,
            options.targetPlatform,
            options.targetArchitecture,
            error,
          )
        )
          continue;
        throw new Error(`${label} unpacked regular file is missing: ${entry}`, { cause: error });
      }
      if (
        !physicalMetadata.isFile() ||
        physicalMetadata.isSymbolicLink() ||
        physicalMetadata.size !== metadata.size
      ) {
        throw new Error(`${label} unpacked regular file is invalid: ${entry}`);
      }
    } else if (dataOffset + BigInt(metadata.offset) + BigInt(metadata.size) > archiveSize) {
      throw new Error(`${label} regular file extends beyond the archive: ${entry}`);
    }

    let bytes;
    try {
      bytes = extractFile(archivePath, archiveEntry, false);
    } catch (error) {
      throw new Error(`${label} regular file cannot be extracted: ${entry}`, { cause: error });
    }
    if (bytes.length !== metadata.size) {
      throw new Error(`${label} regular file size is invalid: ${entry}`);
    }
    yield { entry, bytes, metadata };
  }
}

// after-pack.cjs prunes ONNX binaries after electron-builder has written the ASAR header.
// The stale unpacked metadata is valid only for a known binary outside the package target.
function isAllowedPrunedOnnxNative(entry, targetPlatform, targetArchitecture, error) {
  if (error?.code !== 'ENOENT' && error?.code !== 'ENOTDIR') return false;
  const currentAsarPlatform = TARGET_ASAR_PLATFORM[targetPlatform];
  if (currentAsarPlatform === undefined || !['arm64', 'x64'].includes(targetArchitecture ?? ''))
    return false;
  const match =
    /^node_modules\/onnxruntime-node\/bin\/napi-v3\/(darwin|linux|win32)\/(arm64|x64)\/([^/]+)$/u.exec(
      entry,
    );
  if (match === null) return false;
  const [, asarPlatform, architecture, filename] = match;
  return (
    (asarPlatform !== currentAsarPlatform || architecture !== targetArchitecture) &&
    ONNX_NATIVE_FILENAMES[asarPlatform].has(filename)
  );
}

function asarEntryKind(metadata) {
  if (metadata === null || typeof metadata !== 'object' || Array.isArray(metadata))
    return 'invalid';
  const directory = Object.hasOwn(metadata, 'files');
  const link = Object.hasOwn(metadata, 'link');
  const file = Object.hasOwn(metadata, 'size');
  if (Number(directory) + Number(link) + Number(file) !== 1) return 'invalid';
  if (directory) {
    return metadata.files !== null &&
      typeof metadata.files === 'object' &&
      !Array.isArray(metadata.files)
      ? 'directory'
      : 'invalid';
  }
  if (link)
    return typeof metadata.link === 'string' && metadata.link.length > 0 ? 'link' : 'invalid';
  if (!Number.isSafeInteger(metadata.size) || metadata.size < 0) return 'invalid';
  if (Object.hasOwn(metadata, 'unpacked') && typeof metadata.unpacked !== 'boolean')
    return 'invalid';
  if (metadata.unpacked === true) return 'file';
  return typeof metadata.offset === 'string' && /^(?:0|[1-9][0-9]*)$/u.test(metadata.offset)
    ? 'file'
    : 'invalid';
}
