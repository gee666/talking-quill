import { extractFile, getRawHeader, statFile } from '@electron/asar';
import { lstatSync, readdirSync, statSync } from 'node:fs';
import { relative, resolve, sep } from 'node:path';

export function* extractRegularAsarFiles(archivePath, entries, label = 'ASAR') {
  verifyPhysicalUnpackedFiles(archivePath, label);
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

function verifyPhysicalUnpackedFiles(archivePath, label) {
  const root = `${archivePath}.unpacked`;
  let rootMetadata;
  try {
    rootMetadata = lstatSync(root);
  } catch (error) {
    if (error?.code === 'ENOENT') return;
    throw new Error(`${label} unpacked root cannot be inspected`, { cause: error });
  }
  if (!rootMetadata.isDirectory() || rootMetadata.isSymbolicLink()) {
    throw new Error(`${label} unpacked root is not a physical directory`);
  }
  function walk(directory) {
    for (const entry of readdirSync(directory, { withFileTypes: true })) {
      const absolute = resolve(directory, entry.name);
      const physicalMetadata = lstatSync(absolute);
      const name = relative(root, absolute).split(sep).join('/');
      if (physicalMetadata.isSymbolicLink()) {
        throw new Error(`${label} unpacked physical link is not allowed: ${name}`);
      }
      if (physicalMetadata.isDirectory()) {
        walk(absolute);
        continue;
      }
      if (!physicalMetadata.isFile()) {
        throw new Error(`${label} unpacked physical entry is invalid: ${name}`);
      }
      let metadata;
      try {
        metadata = statFile(archivePath, name.split('/').join(sep), false);
      } catch (error) {
        throw new Error(`${label} contains an unexpected unpacked physical file: ${name}`, {
          cause: error,
        });
      }
      if (
        asarEntryKind(metadata) !== 'file' ||
        metadata.unpacked !== true ||
        metadata.size !== physicalMetadata.size
      ) {
        throw new Error(`${label} unpacked physical file does not match metadata: ${name}`);
      }
    }
  }
  walk(root);
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
