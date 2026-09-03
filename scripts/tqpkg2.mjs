import { createHash } from 'node:crypto';
import { zstdDecompressSync } from 'node:zlib';

export const TQPKG2 = Object.freeze({
  footerSize: 128,
  manifestMax: 8 * 1024 * 1024,
  filesMax: 200_000,
  fileMax: 4 * 1024 ** 3,
  treeMax: 16 * 1024 ** 3,
});
const digest = (bytes) => createHash('sha256').update(bytes).digest();
const hex = (value) => /^[0-9a-f]{64}$/u.test(value ?? '');
const exactKeys = (value, keys) =>
  value !== null &&
  typeof value === 'object' &&
  !Array.isArray(value) &&
  Object.keys(value).sort().join('\0') === [...keys].sort().join('\0');

export function canonicalJson(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean')
    return JSON.stringify(value);
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value) || value < 0) throw new Error('TQPKG2 number is invalid');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (typeof value !== 'object') throw new Error('TQPKG2 JSON value is invalid');
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`;
}

export function validateTqpkg2Path(path) {
  if (
    !path ||
    !/^[\x20-\x7e]+$/u.test(path) ||
    path.startsWith('/') ||
    path.includes('\\') ||
    path.includes(':') ||
    path.length > 1024
  )
    throw new Error(`invalid TQPKG2 path: ${path}`);
  const reserved = /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/iu;
  for (const part of path.split('/'))
    if (
      !part ||
      Buffer.byteLength(part) > 255 ||
      part === '.' ||
      part === '..' ||
      part.endsWith('.') ||
      part.endsWith(' ') ||
      reserved.test(part) ||
      /[<>"|?*]/u.test(part) ||
      [...part].some((character) => character.codePointAt(0) < 32)
    )
      throw new Error(`invalid TQPKG2 path: ${path}`);
}

export function tqpkg2TreeDigest(files) {
  const tree = createHash('sha256');
  for (const file of files)
    for (const value of [file.path, String(file.mode), String(file.size), file.sha256]) {
      const bytes = Buffer.from(value);
      const length = Buffer.alloc(8);
      length.writeBigUInt64LE(BigInt(bytes.length));
      tree.update(length).update(bytes);
    }
  return tree.digest('hex');
}

export function zstdFrameLength(bytes) {
  if (bytes.length < 6 || bytes.readUInt32LE(0) !== 0xfd2fb528)
    throw new Error('TQPKG2 block is not one Zstandard frame');
  const descriptor = bytes[4];
  if (descriptor === undefined || (descriptor & 0x18) !== 0)
    throw new Error('TQPKG2 Zstandard frame header is reserved');
  const single = (descriptor & 0x20) !== 0,
    checksum = (descriptor & 0x04) !== 0;
  let offset = 5 + (single ? 0 : 1);
  const dictionaryLength = [0, 1, 2, 4][descriptor & 3];
  const sizeFlag = descriptor >> 6;
  const contentLength =
    sizeFlag === 0 ? (single ? 1 : 0) : sizeFlag === 1 ? 2 : sizeFlag === 2 ? 4 : 8;
  offset += dictionaryLength + contentLength;
  while (true) {
    if (offset + 3 > bytes.length) throw new Error('TQPKG2 Zstandard block header is truncated');
    const header = bytes.readUIntLE(offset, 3);
    offset += 3;
    const last = (header & 1) !== 0,
      type = (header >> 1) & 3,
      size = header >> 3;
    if (type === 3) throw new Error('TQPKG2 Zstandard block type is reserved');
    offset += type === 1 ? 1 : size;
    if (offset > bytes.length) throw new Error('TQPKG2 Zstandard block is truncated');
    if (last) break;
  }
  return offset + (checksum ? 4 : 0);
}

export function parseTqpkg2(
  bytes,
  expectedArchitecture,
  {
    allowAcceptanceFaults = false,
    allowAcceptanceRepair = allowAcceptanceFaults,
    allowStaleSchema2Cleanup = false,
  } = {},
) {
  if (!Buffer.isBuffer(bytes) || bytes.length < 384 || bytes.readUInt16LE(0) !== 0x5a4d)
    throw new Error('TQPKG2 image is not PE');
  const pe = bytes.readUInt32LE(0x3c),
    optional = pe + 24;
  const magic = bytes.readUInt16LE(optional),
    directory = magic === 0x20b ? optional + 112 : magic === 0x10b ? optional + 96 : -1;
  if (
    directory < 0 ||
    bytes.readUInt32LE(pe) !== 0x00004550 ||
    bytes.readUInt16LE(pe + 24 + 68) !== 2
  )
    throw new Error('TQPKG2 PE header is invalid');
  const machine =
    expectedArchitecture === 'x64' ? 0x8664 : expectedArchitecture === 'arm64' ? 0xaa64 : -1;
  if (bytes.readUInt16LE(pe + 4) !== machine) throw new Error('TQPKG2 PE architecture is invalid');
  const certificateOffset = bytes.readUInt32LE(directory + 32),
    certificateSize = bytes.readUInt32LE(directory + 36);
  const end =
    certificateOffset === 0 && certificateSize === 0
      ? bytes.length
      : certificateSize >= 8 && certificateOffset + certificateSize === bytes.length
        ? certificateOffset
        : -1;
  const footerOffset = end - TQPKG2.footerSize,
    footer = bytes.subarray(footerOffset, end);
  if (
    footerOffset < 256 ||
    footer.subarray(0, 8).toString('binary') !== 'TQPKG2\0\0' ||
    footer.readUInt32LE(8) !== 2 ||
    footer.readUInt32LE(12) !== 0 ||
    footer.subarray(104).some((byte) => byte !== 0)
  )
    throw new Error('TQPKG2 footer is invalid');
  const offset = Number(footer.readBigUInt64LE(16)),
    size = Number(footer.readBigUInt64LE(24)),
    manifestSize = Number(footer.readBigUInt64LE(32));
  if (
    ![offset, size, manifestSize].every(Number.isSafeInteger) ||
    size <= 0 ||
    manifestSize <= 0 ||
    manifestSize > Math.min(size, TQPKG2.manifestMax) ||
    offset + size !== footerOffset
  )
    throw new Error('TQPKG2 range is invalid');
  const packageBytes = bytes.subarray(offset, footerOffset),
    manifestBytes = packageBytes.subarray(0, manifestSize);
  if (
    !digest(packageBytes).equals(footer.subarray(40, 72)) ||
    !digest(manifestBytes).equals(footer.subarray(72, 104))
  )
    throw new Error('TQPKG2 package digest is invalid');
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  if (
    !exactKeys(manifest, [
      'architecture',
      'faultPhase',
      'files',
      'packageMode',
      'predecessor',
      'schemaVersion',
      'sourceCommit',
      'sourceTree',
      'target',
      'treeSha256',
      'version',
    ]) ||
    !exactKeys(manifest.target, [
      'gatewaySha256',
      'ownerSha256',
      'recoveryLauncherSha256',
      'releaseBuildDigest',
    ]) ||
    Buffer.from(canonicalJson(manifest)).compare(manifestBytes) !== 0 ||
    manifest.schemaVersion !== 2 ||
    manifest.architecture !== expectedArchitecture ||
    !/^\d+\.\d+\.\d+$/u.test(manifest.version ?? '') ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceTree ?? '') ||
    ![
      'fresh',
      'update',
      ...(allowAcceptanceRepair ? ['repair'] : []),
      ...(allowStaleSchema2Cleanup ? ['stale-schema2-cleanup'] : []),
    ].includes(manifest.packageMode) ||
    (manifest.packageMode === 'update') !== (manifest.predecessor !== null) ||
    (manifest.packageMode === 'stale-schema2-cleanup' &&
      (manifest.predecessor !== null || manifest.faultPhase !== null)) ||
    (manifest.predecessor !== null &&
      (!exactKeys(manifest.predecessor, [
        'gatewaySha256',
        'ownerSha256',
        'releaseBuildDigest',
        'version',
      ]) ||
        !/^\d+\.\d+\.\d+$/u.test(manifest.predecessor.version ?? '') ||
        !hex(manifest.predecessor.releaseBuildDigest) ||
        !hex(manifest.predecessor.gatewaySha256) ||
        !hex(manifest.predecessor.ownerSha256))) ||
    !hex(manifest.treeSha256) ||
    !hex(manifest.target?.releaseBuildDigest) ||
    !hex(manifest.target?.gatewaySha256) ||
    !hex(manifest.target?.ownerSha256) ||
    !hex(manifest.target?.recoveryLauncherSha256) ||
    (manifest.faultPhase !== null &&
      (manifest.packageMode !== 'repair' ||
        !allowAcceptanceFaults ||
        ![
          'staged',
          'prepared',
          'predecessorMoved',
          'publishing',
          'publishedBeforePersist',
          'published',
          'registered',
          'committed',
          'legacyRetiring',
          'legacyRetired',
          'terminalAcceptance',
        ].includes(manifest.faultPhase))) ||
    !Array.isArray(manifest.files) ||
    manifest.files.length === 0 ||
    manifest.files.length > TQPKG2.filesMax
  )
    throw new Error('TQPKG2 manifest identity is invalid');
  const names = new Set(),
    contents = new Map();
  let expectedOffset = manifestSize,
    total = 0;
  for (const file of manifest.files) {
    if (!exactKeys(file, ['blockOffset', 'blockSize', 'mode', 'path', 'sha256', 'size']))
      throw new Error('TQPKG2 file schema is invalid');
    validateTqpkg2Path(file.path);
    const name = file.path.toLowerCase();
    if (names.has(name)) throw new Error('TQPKG2 path collision');
    names.add(name);
    if (
      file.mode !== 0 ||
      !Number.isSafeInteger(file.size) ||
      file.size < 0 ||
      file.size > TQPKG2.fileMax ||
      file.blockOffset !== expectedOffset ||
      !Number.isSafeInteger(file.blockSize) ||
      file.blockSize <= 0 ||
      !hex(file.sha256)
    )
      throw new Error('TQPKG2 block framing is invalid');
    expectedOffset += file.blockSize;
    total += file.size;
    if (expectedOffset > size || total > TQPKG2.treeMax)
      throw new Error('TQPKG2 size limit exceeded');
    const compressed = packageBytes.subarray(file.blockOffset, expectedOffset);
    if (zstdFrameLength(compressed) !== compressed.length)
      throw new Error('TQPKG2 compressed frame has trailing bytes');
    const content = zstdDecompressSync(compressed, { maxOutputLength: file.size });
    if (content.length !== file.size || digest(content).toString('hex') !== file.sha256)
      throw new Error('TQPKG2 file digest is invalid');
    contents.set(file.path, content);
  }
  if (expectedOffset !== size || tqpkg2TreeDigest(manifest.files) !== manifest.treeSha256)
    throw new Error('TQPKG2 tree digest is invalid');
  const targetFiles = [
    ['resources/helper/talking-quill-helper.exe', manifest.target.gatewaySha256],
    ['resources/helper/talking-quill-keyboard-owner.exe', manifest.target.ownerSha256],
    [
      'resources/helper/talking-quill-update-recovery-launcher.exe',
      manifest.target.recoveryLauncherSha256,
    ],
  ];
  for (const [path, sha256] of targetFiles) {
    const matches = manifest.files.filter((file) => file.path === path && file.sha256 === sha256);
    if (matches.length !== 1) throw new Error('TQPKG2 target native role is missing or ambiguous');
  }
  return { manifest, contents, packageOffset: offset, packageSize: size, manifestSize };
}

export function bindTqpkg2OwnerManifest(manifest, owner) {
  const role = (name) => owner.roles?.find((value) => value.role === name)?.sha256;
  const predecessor = (left, right) =>
    left == null || right == null
      ? left == null && right == null
      : ['version', 'releaseBuildDigest', 'gatewaySha256', 'ownerSha256'].every(
          (key) => left[key] === right[key],
        );
  if (
    owner.version !== manifest.version ||
    owner.architecture !== manifest.architecture ||
    owner.sourceCommit !== manifest.sourceCommit ||
    owner.sourceTree !== manifest.sourceTree ||
    owner.releaseBuildDigest !== manifest.target.releaseBuildDigest ||
    role('gateway') !== manifest.target.gatewaySha256 ||
    role('owner') !== manifest.target.ownerSha256 ||
    role('recovery-launcher') !== manifest.target.recoveryLauncherSha256 ||
    !predecessor(owner.predecessor, manifest.predecessor)
  )
    throw new Error('TQPKG2 identity is not bound to owner release manifest');
}
