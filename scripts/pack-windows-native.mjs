import { createHash } from 'node:crypto';
import { readFile, readdir, lstat, writeFile, rm, rename, stat } from 'node:fs/promises';
import { basename, dirname, join, relative, resolve, sep } from 'node:path';
import { zstdCompressSync, constants as zlibConstants } from 'node:zlib';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { dump, load } from 'js-yaml';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: pack-windows-native.mjs <x64|arm64> [output-directory]');
}
const packageJson = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'));
const outputDirectory = resolve(root, process.argv[3] ?? 'release');
const input = resolve(
  outputDirectory,
  architecture === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked',
);
const output = resolve(
  outputDirectory,
  `Talking-Quill-${packageJson.version}-win-${architecture}.exe`,
);
const stub = resolve(root, 'tmp', 'windows-setup', architecture, 'talking-quill-windows-setup.exe');
const packageMode = process.env.TALKING_QUILL_PACKAGE_MODE ?? 'update';
const sourceCommit = process.env.TALKING_QUILL_RELEASE_COMMIT ?? '';
const sourceTree = process.env.TALKING_QUILL_RELEASE_TREE ?? '';
if (!/^[0-9a-f]{40}$/u.test(sourceCommit) || !/^[0-9a-f]{40}$/u.test(sourceTree)) {
  throw new Error('native package requires full source commit and tree digests');
}
const paths = await collect(input);
const sourceManifest = JSON.parse(
  await readFile(resolve(input, 'resources', 'keyboard-owner-release-v1.json'), 'utf8'),
);
const predecessor = ['update', 'release'].includes(packageMode) ? sourceManifest.predecessor : null;
if (['update', 'release'].includes(packageMode) !== (predecessor !== null && predecessor !== undefined)) {
  throw new Error('native update package requires one exact predecessor');
}
const blocks = [];
const files = [];
for (const item of paths) {
  const bytes = await readFile(item.absolute);
  const compressed = zstdCompressSync(bytes, {
    params: {
      [zlibConstants.ZSTD_c_compressionLevel]: 19,
      [zlibConstants.ZSTD_c_checksumFlag]: 1,
      [zlibConstants.ZSTD_c_contentSizeFlag]: 1,
      [zlibConstants.ZSTD_c_nbWorkers]: 0,
    },
  });
  blocks.push(compressed);
  files.push({
    path: item.path,
    mode: 0,
    size: bytes.length,
    sha256: hash(bytes),
    blockOffset: 0,
    blockSize: compressed.length,
  });
}
const tree = createHash('sha256');
for (const file of files) {
  for (const value of [file.path, String(file.mode), String(file.size), file.sha256])
    frame(tree, value);
}
const manifest = {
  architecture,
  files,
  packageMode,
  predecessor:
    predecessor === null || predecessor === undefined
      ? null
      : {
          gatewaySha256: predecessor.gatewaySha256,
          ownerSha256: predecessor.ownerSha256,
          releaseBuildDigest: predecessor.releaseBuildDigest,
          version: predecessor.version,
        },
  schemaVersion: 2,
  sourceCommit,
  sourceTree,
  treeSha256: tree.digest('hex'),
  version: packageJson.version,
};
let manifestBytes;
for (let iteration = 0; iteration < 4; iteration += 1) {
  manifestBytes = Buffer.from(canonicalJson(manifest));
  let offset = manifestBytes.length;
  for (const file of files) {
    file.blockOffset = offset;
    offset += file.blockSize;
  }
}
manifestBytes = Buffer.from(canonicalJson(manifest));
const expectedEnd = files.at(-1).blockOffset + files.at(-1).blockSize;
if (expectedEnd !== manifestBytes.length + blocks.reduce((sum, block) => sum + block.length, 0)) {
  throw new Error('native manifest offsets did not converge');
}
const packageBytes = Buffer.concat([manifestBytes, ...blocks]);
const stubBytes = await readFile(stub);
requireGuiPe(stubBytes, architecture);
const footer = Buffer.alloc(128);
footer.write('TQPKG2\0\0', 0, 'ascii');
footer.writeUInt32LE(2, 8);
footer.writeBigUInt64LE(BigInt(stubBytes.length), 16);
footer.writeBigUInt64LE(BigInt(packageBytes.length), 24);
footer.writeBigUInt64LE(BigInt(manifestBytes.length), 32);
Buffer.from(hash(packageBytes), 'hex').copy(footer, 40);
Buffer.from(hash(manifestBytes), 'hex').copy(footer, 72);
const pending = `${output}.pending-native-package`;
await rm(pending, { force: true });
await writeFile(pending, Buffer.concat([stubBytes, packageBytes, footer]), { flag: 'wx' });
await rm(output, { force: true });
await rename(pending, output);
await rebuildUpdateMetadata(output);
console.log(`Packed ${paths.length} files into ${output}`);

async function collect(directory) {
  const result = [];
  async function visit(current) {
    const entries = await readdir(current, { withFileTypes: true });
    entries.sort((left, right) => Buffer.from(left.name).compare(Buffer.from(right.name)));
    for (const entry of entries) {
      const absolute = resolve(current, entry.name);
      const metadata = await lstat(absolute);
      if (metadata.isSymbolicLink()) throw new Error(`native package rejects link: ${absolute}`);
      if (metadata.isDirectory()) await visit(absolute);
      else if (metadata.isFile()) {
        const path = relative(directory, absolute).split(sep).join('/');
        validatePath(path);
        result.push({ absolute, path });
      } else throw new Error(`native package rejects special entry: ${absolute}`);
    }
  }
  await visit(directory);
  const folded = new Set();
  for (const item of result) {
    const key = item.path.toLowerCase();
    if (folded.has(key)) throw new Error(`native package has a case collision: ${item.path}`);
    folded.add(key);
  }
  return result;
}

export function validatePath(path) {
  if (
    !path ||
    !/^[\x20-\x7e]+$/u.test(path) ||
    path.startsWith('/') ||
    path.includes('\\') ||
    path.includes(':') ||
    path.length > 1024
  )
    throw new Error(`invalid native package path: ${path}`);
  const reserved = /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/iu;
  for (const part of path.split('/')) {
    if (
      !part ||
      part === '.' ||
      part === '..' ||
      part.endsWith('.') ||
      part.endsWith(' ') ||
      reserved.test(part) ||
      /[<>"|?*]/u.test(part) ||
      [...part].some((character) => character.codePointAt(0) < 32)
    )
      throw new Error(`invalid native package path: ${path}`);
  }
}

function canonicalJson(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean')
    return JSON.stringify(value);
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0)
      throw new Error('native manifest number is invalid');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`;
}

function frame(digest, value) {
  const bytes = Buffer.from(value);
  const length = Buffer.alloc(8);
  length.writeBigUInt64LE(BigInt(bytes.length));
  digest.update(length).update(bytes);
}
function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function requireGuiPe(bytes, expectedArchitecture) {
  if (bytes.length < 256 || bytes.readUInt16LE(0) !== 0x5a4d)
    throw new Error('native setup stub is not PE');
  const pe = bytes.readUInt32LE(0x3c);
  const machine = expectedArchitecture === 'x64' ? 0x8664 : 0xaa64;
  if (
    bytes.readUInt32LE(pe) !== 0x00004550 ||
    bytes.readUInt16LE(pe + 4) !== machine ||
    bytes.readUInt16LE(pe + 24 + 68) !== 2
  )
    throw new Error('native setup stub architecture or GUI subsystem is invalid');
}

async function rebuildUpdateMetadata(path) {
  const require = createRequire(import.meta.url);
  const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
  const appBuilderRoot = dirname(electronBuilderRequire.resolve('app-builder-lib/package.json'));
  const { buildBlockMap } = electronBuilderRequire(
    join(appBuilderRoot, 'out', 'targets', 'blockmap', 'blockmap.js'),
  );
  const blockmap = `${path}.blockmap`;
  await rm(blockmap, { force: true });
  await buildBlockMap(path, 'gzip', blockmap);
  const [bytes, blockmapMetadata] = await Promise.all([readFile(path), stat(blockmap)]);
  const sha512 = createHash('sha512').update(bytes).digest('base64');
  let metadataWritten = false;
  for (const name of await readdir(dirname(path))) {
    if (!/^latest(?:-[^.]+)?\.ya?ml$/iu.test(name)) continue;
    const metadataPath = resolve(dirname(path), name);
    const metadata = load(await readFile(metadataPath, 'utf8'));
    let changed = false;
    for (const file of Array.isArray(metadata?.files) ? metadata.files : []) {
      if (basename(file.url ?? file.path ?? '') !== basename(path)) continue;
      Object.assign(file, { sha512, size: bytes.length, blockMapSize: blockmapMetadata.size });
      changed = true;
    }
    if (basename(metadata?.path ?? '') === basename(path)) {
      Object.assign(metadata, { sha512, size: bytes.length });
      changed = true;
    }
    if (changed) {
      await writeFile(metadataPath, dump(metadata, { lineWidth: -1 }), 'utf8');
      metadataWritten = true;
    }
  }
  if (!metadataWritten) {
    const metadata = {
      version: packageJson.version,
      files: [
        { url: basename(path), sha512, size: bytes.length, blockMapSize: blockmapMetadata.size },
      ],
      path: basename(path),
      sha512,
      size: bytes.length,
    };
    await writeFile(
      resolve(dirname(path), `latest-${architecture}.yml`),
      dump(metadata, { lineWidth: -1, sortKeys: true }),
      'utf8',
    );
  }
}
