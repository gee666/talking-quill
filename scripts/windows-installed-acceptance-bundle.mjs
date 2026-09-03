import { createHash } from 'node:crypto';
import {
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  realpath,
  rm,
  stat,
  writeFile,
} from 'node:fs/promises';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';

const MANIFEST_NAME = 'bundle-manifest.json';
const EVIDENCE_NAME = 'evidence-input.json';
const MAX_ENTRIES = 200_000;
const MAX_ARCHIVE_BYTES = 0xffff_ffff;
const FIXED_MODE = 0x8180_0000;
const UTF8_FLAG = 0x0800;
const DOS_TIME = 0;
const DOS_DATE = 0x0021;

export async function verifyAcceptanceBundleTree(rootPath, expected = {}) {
  const root = resolve(rootPath);
  await assertNoLinkPath(root, { directory: true });
  const manifestBytes = await readStrictRegularFile(
    resolve(root, MANIFEST_NAME),
    'acceptance bundle manifest',
  );
  if (expected.manifestSha256 !== undefined && sha256(manifestBytes) !== expected.manifestSha256) {
    throw new Error('Acceptance bundle manifest SHA-256 does not match authorized extraction');
  }
  const manifest = parseManifest(manifestBytes, expected);
  const files = await collectTree(root);
  const expectedPaths = [...manifest.entries.map(({ path }) => path), MANIFEST_NAME].sort(compare);
  if (
    canonicalAcceptanceJson(files.map(({ path }) => path)) !==
    canonicalAcceptanceJson(expectedPaths)
  ) {
    throw new Error('Acceptance bundle has missing or extra files');
  }
  const declarations = new Map(manifest.entries.map((entry) => [entry.path, entry]));
  for (const file of files) {
    if (file.path === MANIFEST_NAME) continue;
    const declaration = declarations.get(file.path);
    if (
      declaration === undefined ||
      declaration.bytes !== file.bytes.length ||
      declaration.sha256 !== sha256(file.bytes)
    ) {
      throw new Error(`Acceptance bundle file identity mismatch: ${file.path}`);
    }
  }
  const evidence = parseCanonicalJson(
    files.find(({ path }) => path === EVIDENCE_NAME)?.bytes,
    'acceptance evidence input',
  );
  if (
    evidence.architecture !== manifest.architecture ||
    evidence.outputPath !== '../evidence.json' ||
    evidence.acceptance?.sourceRevision !== manifest.sourceCommit.slice(0, 12)
  ) {
    throw new Error('Acceptance bundle evidence architecture/source/output binding is invalid');
  }
  verifyEvidenceReferences(evidence, declarations);
  verifyCanonicalReleaseReference(evidence.canonicalRelease, manifest, declarations);
  const manifestSha256 = sha256(manifestBytes);
  if (
    sha256(
      await readStrictRegularFile(
        resolve(root, MANIFEST_NAME),
        'acceptance bundle manifest recheck',
      ),
    ) !== manifestSha256
  ) {
    throw new Error('Acceptance bundle manifest changed during verification');
  }
  return Object.freeze({ root, manifest, evidence, manifestSha256 });
}

export async function createDeterministicAcceptanceZip(rootPath, outputPath, expected = {}) {
  const verified = await verifyAcceptanceBundleTree(rootPath, expected);
  const files = await collectTree(verified.root);
  const localParts = [];
  const centralParts = [];
  let offset = 0;
  for (const file of files) {
    const name = Buffer.from(file.path, 'utf8');
    const crc = crc32(file.bytes);
    const local = Buffer.alloc(30);
    local.writeUInt32LE(0x0403_4b50, 0);
    local.writeUInt16LE(20, 4);
    local.writeUInt16LE(UTF8_FLAG, 6);
    local.writeUInt16LE(0, 8);
    local.writeUInt16LE(DOS_TIME, 10);
    local.writeUInt16LE(DOS_DATE, 12);
    local.writeUInt32LE(crc, 14);
    local.writeUInt32LE(file.bytes.length, 18);
    local.writeUInt32LE(file.bytes.length, 22);
    local.writeUInt16LE(name.length, 26);
    local.writeUInt16LE(0, 28);
    localParts.push(local, name, file.bytes);

    const central = Buffer.alloc(46);
    central.writeUInt32LE(0x0201_4b50, 0);
    central.writeUInt16LE(0x0314, 4);
    central.writeUInt16LE(20, 6);
    central.writeUInt16LE(UTF8_FLAG, 8);
    central.writeUInt16LE(0, 10);
    central.writeUInt16LE(DOS_TIME, 12);
    central.writeUInt16LE(DOS_DATE, 14);
    central.writeUInt32LE(crc, 16);
    central.writeUInt32LE(file.bytes.length, 20);
    central.writeUInt32LE(file.bytes.length, 24);
    central.writeUInt16LE(name.length, 28);
    central.writeUInt16LE(0, 30);
    central.writeUInt16LE(0, 32);
    central.writeUInt16LE(0, 34);
    central.writeUInt16LE(0, 36);
    central.writeUInt32LE(FIXED_MODE, 38);
    central.writeUInt32LE(offset, 42);
    centralParts.push(central, name);
    offset += local.length + name.length + file.bytes.length;
    if (offset > MAX_ARCHIVE_BYTES) throw new Error('Acceptance ZIP exceeds ZIP32 limits');
  }
  const centralOffset = offset;
  const centralBytes = Buffer.concat(centralParts);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x0605_4b50, 0);
  end.writeUInt16LE(0, 4);
  end.writeUInt16LE(0, 6);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(centralBytes.length, 12);
  end.writeUInt32LE(centralOffset, 16);
  end.writeUInt16LE(0, 20);
  const archive = Buffer.concat([...localParts, centralBytes, end]);
  const destination = resolve(outputPath);
  await assertNoLinkPath(dirname(destination), { directory: true });
  await writeFile(destination, archive, { flag: 'wx', mode: 0o600 });
  const parsed = await verifyAcceptanceBundleArchive(destination, expected);
  return Object.freeze({
    path: destination,
    bytes: archive.length,
    sha256: sha256(archive),
    parsed,
  });
}

export async function verifyAcceptanceBundleArchive(archivePath, expected = {}) {
  await assertNoLinkPath(resolve(archivePath), { file: true });
  const archive = await readFile(resolve(archivePath));
  if (expected.bundleSha256 !== undefined && sha256(archive) !== expected.bundleSha256) {
    throw new Error('Acceptance ZIP SHA-256 does not match authorization');
  }
  const entries = parseZip(archive);
  const manifestEntry = entries.find(({ path }) => path === MANIFEST_NAME);
  if (manifestEntry === undefined) throw new Error('Acceptance ZIP manifest is missing');
  const manifest = parseManifest(manifestEntry.bytes, expected);
  const expectedPaths = [...manifest.entries.map(({ path }) => path), MANIFEST_NAME].sort(compare);
  if (
    canonicalAcceptanceJson(entries.map(({ path }) => path)) !==
    canonicalAcceptanceJson(expectedPaths)
  ) {
    throw new Error('Acceptance ZIP has missing or extra entries');
  }
  const declarations = new Map(manifest.entries.map((entry) => [entry.path, entry]));
  for (const entry of entries) {
    if (entry.path === MANIFEST_NAME) continue;
    const declaration = declarations.get(entry.path);
    if (
      declaration === undefined ||
      declaration.bytes !== entry.bytes.length ||
      declaration.sha256 !== sha256(entry.bytes)
    ) {
      throw new Error(`Acceptance ZIP entry identity mismatch: ${entry.path}`);
    }
  }
  return Object.freeze({ archivePath: resolve(archivePath), manifest, entries });
}

export async function extractVerifiedAcceptanceBundle(archivePath, destination, expected = {}) {
  const verified = await verifyAcceptanceBundleArchive(archivePath, expected);
  const root = resolve(destination);
  await assertNoLinkPath(dirname(root), { directory: true });
  try {
    await lstat(root);
    throw new Error('Acceptance extraction destination already exists');
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
  await mkdir(root, { mode: 0o700 });
  try {
    for (const entry of verified.entries) {
      const output = resolveBundlePath(root, entry.path);
      await mkdir(dirname(output), { recursive: true, mode: 0o700 });
      await writeFile(output, entry.bytes, { flag: 'wx', mode: 0o600 });
    }
    return await verifyAcceptanceBundleTree(root, expected);
  } catch (error) {
    await rm(root, { recursive: true, force: true }).catch(() => undefined);
    throw error;
  }
}

async function readStrictRegularFile(path, label) {
  const handle = await open(path, 'r');
  try {
    const before = await handle.stat();
    const bytes = await handle.readFile();
    const after = await handle.stat();
    if (
      !before.isFile() ||
      before.nlink !== 1 ||
      before.size !== bytes.length ||
      before.size !== after.size ||
      before.mtimeMs !== after.mtimeMs
    ) {
      throw new Error(`${label} is linked, non-regular, or changed while read`);
    }
    return bytes;
  } finally {
    await handle.close();
  }
}

export async function assertNoLinkPath(path, expectation = {}) {
  const absolute = resolve(path);
  const parsedRoot = resolve(absolute).split(/[\\/]/u);
  let current = parsedRoot.shift() || sep;
  if (/^[A-Za-z]:$/u.test(current)) current += sep;
  for (const component of parsedRoot) {
    if (!component) continue;
    current = resolve(current, component);
    const metadata = await lstat(current);
    if (metadata.isSymbolicLink())
      throw new Error(`Path contains a link or reparse point: ${current}`);
  }
  const metadata = await stat(absolute);
  if (expectation.file && metadata.nlink !== 1)
    throw new Error(`Path is multiply linked: ${absolute}`);
  if (expectation.file && !metadata.isFile())
    throw new Error(`Path is not a regular file: ${absolute}`);
  if (expectation.directory && !metadata.isDirectory())
    throw new Error(`Path is not a directory: ${absolute}`);
  const canonical = await realpath(absolute);
  if (
    process.platform === 'win32'
      ? canonical.toLowerCase() !== absolute.toLowerCase()
      : canonical !== absolute
  ) {
    throw new Error(`Path is not canonical or traverses a link: ${absolute}`);
  }
  return absolute;
}

export function resolveBundlePath(root, path) {
  validateBundlePath(path);
  const output = resolve(root, ...path.split('/'));
  const local = relative(resolve(root), output);
  if (local === '' || local === '..' || local.startsWith(`..${sep}`) || isAbsolute(local)) {
    throw new Error(`Acceptance bundle path escapes its root: ${path}`);
  }
  return output;
}

function parseManifest(bytes, expected) {
  const manifest = parseCanonicalJson(bytes, 'acceptance bundle manifest');
  if (
    Object.keys(manifest).sort().join(',') !==
      'architecture,classification,entries,producerArtifactSetIdentity,schemaVersion,sourceCommit,sourceTree' ||
    manifest.schemaVersion !== 1 ||
    manifest.classification !== 'nonpromotable-installed-acceptance-kit' ||
    !['x64', 'arm64'].includes(manifest.architecture) ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceTree ?? '') ||
    !/^[0-9a-f]{64}$/u.test(manifest.producerArtifactSetIdentity ?? '') ||
    (expected.producerArtifactSetIdentity !== undefined &&
      manifest.producerArtifactSetIdentity !== expected.producerArtifactSetIdentity) ||
    (expected.architecture !== undefined && manifest.architecture !== expected.architecture) ||
    (expected.sourceCommit !== undefined && manifest.sourceCommit !== expected.sourceCommit) ||
    (expected.sourceTree !== undefined && manifest.sourceTree !== expected.sourceTree) ||
    !Array.isArray(manifest.entries) ||
    manifest.entries.length === 0 ||
    manifest.entries.length > MAX_ENTRIES
  ) {
    throw new Error('Acceptance bundle manifest identity is invalid');
  }
  const names = new Set();
  let prior = '';
  for (const entry of manifest.entries) {
    if (
      Object.keys(entry ?? {})
        .sort()
        .join(',') !== 'bytes,path,sha256' ||
      !Number.isSafeInteger(entry.bytes) ||
      entry.bytes < 0 ||
      !/^[0-9a-f]{64}$/u.test(entry.sha256 ?? '')
    ) {
      throw new Error('Acceptance bundle manifest entry is invalid');
    }
    validateBundlePath(entry.path);
    const folded = entry.path.toLowerCase();
    if (
      names.has(folded) ||
      (prior !== '' &&
        (compare(prior, entry.path) >= 0 || folded.startsWith(`${prior.toLowerCase()}/`)))
    ) {
      throw new Error('Acceptance bundle manifest paths collide or are unsorted');
    }
    names.add(folded);
    prior = entry.path;
  }
  if (names.has(MANIFEST_NAME.toLowerCase()) || !names.has(EVIDENCE_NAME.toLowerCase())) {
    throw new Error('Acceptance bundle manifest inventory is invalid');
  }
  return manifest;
}

function parseCanonicalJson(bytes, label) {
  if (!Buffer.isBuffer(bytes)) throw new Error(`${label} is missing`);
  let value;
  try {
    value = JSON.parse(bytes.toString('utf8'));
  } catch {
    throw new Error(`${label} is not JSON`);
  }
  if (bytes.toString('utf8') !== `${canonicalAcceptanceJson(value)}\n`) {
    throw new Error(`${label} is not canonical`);
  }
  return value;
}

function verifyEvidenceReferences(evidence, declarations) {
  const fields = [
    'installerPath',
    'metadataPath',
    'releaseIdentityPath',
    'validationEvidencePath',
    'electronPath',
    'appAsarPath',
  ];
  const artifacts = [
    evidence.artifacts?.predecessor,
    evidence.artifacts?.candidate,
    evidence.artifacts?.fresh,
    evidence.artifacts?.repair,
    evidence.artifacts?.fault,
    ...Object.values(evidence.artifacts?.faults ?? {}),
  ];
  for (const artifact of artifacts) {
    for (const field of fields) {
      if (artifact?.[field] !== undefined) requireDeclared(artifact[field], declarations);
    }
    if (artifact?.unpackedRoot !== undefined) {
      validateBundlePath(`${artifact.unpackedRoot}/placeholder`);
      if (![...declarations.keys()].some((path) => path.startsWith(`${artifact.unpackedRoot}/`))) {
        throw new Error('Acceptance unpacked root is absent from bundle inventory');
      }
    }
  }
  for (const field of [
    'buildManifestPath',
    'signedRequestsPath',
    'syntheticSenderPath',
    'acceptanceBrokerPath',
    'acceptanceBootstrapPath',
    'trustedLauncherPath',
  ]) {
    requireDeclared(evidence.acceptance?.[field], declarations);
  }
}

function verifyCanonicalReleaseReference(canonicalRelease, manifest, declarations) {
  if (canonicalRelease === undefined) return;
  requireDeclared(canonicalRelease.descriptorPath, declarations);
  requireDeclared(canonicalRelease.provenancePath, declarations);
  if (
    declarations.get(canonicalRelease.descriptorPath)?.sha256 !==
      canonicalRelease.descriptorSha256 ||
    declarations.get(canonicalRelease.provenancePath)?.sha256 !==
      canonicalRelease.provenanceSha256 ||
    canonicalRelease.sourceCommit !== manifest.sourceCommit ||
    canonicalRelease.sourceTree !== manifest.sourceTree ||
    !/^[0-9a-f]{64}$/u.test(canonicalRelease.installerSha256 ?? '')
  ) {
    throw new Error('Acceptance canonicalRelease builder provenance is invalid');
  }
}

function requireDeclared(path, declarations) {
  validateBundlePath(path);
  if (!declarations.has(path))
    throw new Error(`Acceptance evidence references an undeclared file: ${path}`);
}

async function collectTree(root) {
  const files = [];
  const folded = new Set();
  const visit = async (directory) => {
    const entries = await readdir(directory, { withFileTypes: true });
    for (const entry of entries) {
      const absolute = resolve(directory, entry.name);
      const metadata = await lstat(absolute);
      if (metadata.isSymbolicLink())
        throw new Error('Acceptance bundle contains a link or reparse point');
      if (metadata.isDirectory()) await visit(absolute);
      else if (metadata.isFile()) {
        if (metadata.nlink !== 1)
          throw new Error('Acceptance bundle contains a multiply linked file');
        const path = relative(root, absolute).split(sep).join('/');
        validateBundlePath(path);
        const lower = path.toLowerCase();
        if (folded.has(lower)) throw new Error('Acceptance bundle contains a case-colliding path');
        folded.add(lower);
        const handle = await open(absolute, 'r');
        try {
          const before = await handle.stat();
          const bytes = await handle.readFile();
          const after = await handle.stat();
          if (
            before.size !== bytes.length ||
            before.size !== after.size ||
            before.mtimeMs !== after.mtimeMs
          ) {
            throw new Error(`Acceptance bundle file changed while read: ${path}`);
          }
          files.push({ path, bytes });
        } finally {
          await handle.close();
        }
      } else throw new Error('Acceptance bundle contains a non-regular entry');
    }
  };
  await visit(root);
  files.sort((left, right) => compare(left.path, right.path));
  return files;
}

function parseZip(bytes) {
  if (bytes.length < 22 || bytes.readUInt32LE(bytes.length - 22) !== 0x0605_4b50) {
    throw new Error('Acceptance ZIP end record is invalid');
  }
  const end = bytes.length - 22;
  if (
    bytes.readUInt16LE(end + 4) !== 0 ||
    bytes.readUInt16LE(end + 6) !== 0 ||
    bytes.readUInt16LE(end + 20) !== 0
  ) {
    throw new Error('Acceptance ZIP disk/comment fields are invalid');
  }
  const count = bytes.readUInt16LE(end + 8);
  const total = bytes.readUInt16LE(end + 10);
  const centralSize = bytes.readUInt32LE(end + 12);
  const centralOffset = bytes.readUInt32LE(end + 16);
  if (
    count === 0 ||
    count !== total ||
    count > MAX_ENTRIES ||
    centralOffset + centralSize !== end
  ) {
    throw new Error('Acceptance ZIP central directory range is invalid');
  }
  const entries = [];
  const folded = new Set();
  let cursor = centralOffset;
  let expectedLocalOffset = 0;
  let prior = '';
  for (let index = 0; index < count; index += 1) {
    if (cursor + 46 > end || bytes.readUInt32LE(cursor) !== 0x0201_4b50)
      throw new Error('Acceptance ZIP central entry is invalid');
    const nameLength = bytes.readUInt16LE(cursor + 28);
    if (
      bytes.readUInt16LE(cursor + 4) !== 0x0314 ||
      bytes.readUInt16LE(cursor + 6) !== 20 ||
      bytes.readUInt16LE(cursor + 8) !== UTF8_FLAG ||
      bytes.readUInt16LE(cursor + 10) !== 0 ||
      bytes.readUInt16LE(cursor + 12) !== DOS_TIME ||
      bytes.readUInt16LE(cursor + 14) !== DOS_DATE ||
      bytes.readUInt16LE(cursor + 30) !== 0 ||
      bytes.readUInt16LE(cursor + 32) !== 0 ||
      bytes.readUInt16LE(cursor + 34) !== 0 ||
      bytes.readUInt16LE(cursor + 36) !== 0 ||
      bytes.readUInt32LE(cursor + 38) !== FIXED_MODE
    )
      throw new Error('Acceptance ZIP metadata is not normalized');
    const name = bytes.subarray(cursor + 46, cursor + 46 + nameLength);
    const path = name.toString('utf8');
    if (!Buffer.from(path).equals(name)) throw new Error('Acceptance ZIP path is not UTF-8');
    validateBundlePath(path);
    const lower = path.toLowerCase();
    if (
      folded.has(lower) ||
      (prior !== '' && (compare(prior, path) >= 0 || lower.startsWith(`${prior.toLowerCase()}/`)))
    )
      throw new Error('Acceptance ZIP paths collide or are unsorted');
    folded.add(lower);
    prior = path;
    const size = bytes.readUInt32LE(cursor + 24);
    if (bytes.readUInt32LE(cursor + 20) !== size)
      throw new Error('Acceptance ZIP entry is compressed');
    const localOffset = bytes.readUInt32LE(cursor + 42);
    if (
      localOffset !== expectedLocalOffset ||
      localOffset + 30 > centralOffset ||
      bytes.readUInt32LE(localOffset) !== 0x0403_4b50
    )
      throw new Error('Acceptance ZIP local entry is invalid');
    const localNameLength = bytes.readUInt16LE(localOffset + 26);
    if (
      bytes.readUInt16LE(localOffset + 4) !== 20 ||
      bytes.readUInt16LE(localOffset + 6) !== UTF8_FLAG ||
      bytes.readUInt16LE(localOffset + 8) !== 0 ||
      bytes.readUInt16LE(localOffset + 10) !== DOS_TIME ||
      bytes.readUInt16LE(localOffset + 12) !== DOS_DATE ||
      bytes.readUInt16LE(localOffset + 28) !== 0 ||
      bytes.readUInt32LE(localOffset + 18) !== size ||
      bytes.readUInt32LE(localOffset + 22) !== size ||
      localNameLength !== nameLength ||
      !bytes.subarray(localOffset + 30, localOffset + 30 + nameLength).equals(name)
    )
      throw new Error('Acceptance ZIP local metadata differs from central metadata');
    const dataStart = localOffset + 30 + nameLength;
    const content = bytes.subarray(dataStart, dataStart + size);
    const crc = bytes.readUInt32LE(cursor + 16);
    if (
      content.length !== size ||
      bytes.readUInt32LE(localOffset + 14) !== crc ||
      crc32(content) !== crc
    ) {
      throw new Error('Acceptance ZIP CRC or size is invalid');
    }
    entries.push({ path, bytes: Buffer.from(content) });
    expectedLocalOffset = dataStart + size;
    cursor += 46 + nameLength;
  }
  if (cursor !== end || expectedLocalOffset !== centralOffset || entries.at(-1) === undefined)
    throw new Error('Acceptance ZIP central directory is malformed');
  return entries;
}

function validateBundlePath(path) {
  if (
    typeof path !== 'string' ||
    path.length === 0 ||
    path.length > 1024 ||
    path.includes('\\') ||
    path.includes(':') ||
    path.startsWith('/') ||
    path.endsWith('/') ||
    path
      .split('/')
      .some(
        (part) =>
          part === '' ||
          part === '.' ||
          part === '..' ||
          part.endsWith('.') ||
          part.endsWith(' ') ||
          /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/iu.test(part),
      )
  )
    throw new Error(`Invalid acceptance bundle path: ${String(path)}`);
}

function compare(left, right) {
  return Buffer.from(left).compare(Buffer.from(right));
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function crc32(bytes) {
  let crc = 0xffff_ffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) crc = (crc >>> 1) ^ (crc & 1 ? 0xedb8_8320 : 0);
  }
  return (crc ^ 0xffff_ffff) >>> 0;
}

async function main() {
  const [operation, ...operands] = process.argv.slice(2);
  const pathCount = operation === 'extract' ? 2 : 1;
  const [first, second] = operands;
  const [
    architecture,
    sourceCommit,
    sourceTree,
    bundleSha256,
    manifestSha256,
    producerArtifactSetIdentity,
  ] = operands.slice(pathCount);
  const expected = Object.fromEntries(
    Object.entries({
      architecture,
      sourceCommit,
      sourceTree,
      bundleSha256,
      manifestSha256,
      producerArtifactSetIdentity,
    }).filter(([, value]) => value !== undefined && value !== '-'),
  );
  const result =
    operation === 'extract'
      ? await extractVerifiedAcceptanceBundle(first, second, expected)
      : operation === 'verify-tree'
        ? await verifyAcceptanceBundleTree(first, expected)
        : operation === 'verify-archive'
          ? await verifyAcceptanceBundleArchive(first, expected)
          : null;
  if (result === null) throw new Error('Expected extract, verify-tree, or verify-archive');
  console.log(
    canonicalAcceptanceJson({
      result: 'passed',
      sourceCommit: result.manifest.sourceCommit,
      manifestSha256: result.manifestSha256,
      producerArtifactSetIdentity: result.manifest.producerArtifactSetIdentity,
    }),
  );
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
