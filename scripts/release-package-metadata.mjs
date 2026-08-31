import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { readFile, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { currentSourceIdentity } from './source-identity.mjs';

export const RELEASE_PACKAGE_METADATA_NAME = 'keyboard-owner-release-v1.json';
const HEX_32 = /^[0-9a-f]{64}$/u;
const WINDOWS_UPDATE_PUBLIC_KEY_PATH = resolve(
  import.meta.dirname,
  '..',
  'build',
  'windows-update-public-key.sec1',
);
const ROLE_LAYOUT = Object.freeze({
  win: Object.freeze([
    ['gateway', 'resources/helper/talking-quill-helper.exe', false],
    ['owner', 'resources/helper/talking-quill-keyboard-owner.exe', true],
  ]),
  mac: Object.freeze([
    ['gateway', 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper', false],
    [
      'owner',
      'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
      true,
    ],
    ['authority', 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge', false],
  ]),
});

export async function createPackageReleaseMetadata({
  version,
  platform,
  architecture,
  packageRoot,
  releaseBuildDigest,
  sourceIdentity = currentSourceIdentity(),
  outerIdentity,
  freshInstall = process.env.TALKING_QUILL_PERSONAL_FRESH_INSTALL === '1',
  packageMode = process.env.TALKING_QUILL_PACKAGE_MODE ?? (freshInstall ? 'fresh' : 'update'),
  predecessor = packageMode === 'update' ? readPackagePredecessor(process.env, platform, architecture) : null,
}) {
  requireIdentity(version, platform, architecture);
  requireSourceIdentity(sourceIdentity);
  const layout = ROLE_LAYOUT[platform];
  const roles = await Promise.all(
    layout.map(async ([role, path, suppressionCapable]) => ({
      role,
      path,
      sha256: sha256(await readFile(resolve(packageRoot, path))),
      suppressionCapable,
    })),
  );
  if (!['fresh', 'update', 'repair'].includes(packageMode)) {
    throw new Error('Package mode must be explicitly fresh, update, or repair');
  }
  if ((packageMode === 'fresh') !== freshInstall) {
    throw new Error('Package mode and fresh-install marker disagree');
  }
  if (freshInstall && predecessor !== null) {
    throw new Error('A fresh personal-use package cannot declare predecessor metadata');
  }
  const identity = {
    version,
    platform,
    architecture,
    ownerMode: 'local-unsigned-enabled',
    packageMode,
    sourceCommit: sourceIdentity.sourceCommit,
    sourceTree: sourceIdentity.sourceTree,
    roles,
    predecessor,
    ...(platform === 'mac' ? { outerIdentity } : {}),
    ...(freshInstall ? { freshInstall: true } : {}),
  };
  const derived = digestCanonicalIdentity(identity);
  if (releaseBuildDigest !== undefined && releaseBuildDigest !== derived) {
    // Only macOS has a richer policy-signing identity transcript. Windows'
    // release identity is exactly this complete local package layout.
    requireDigest(releaseBuildDigest, 'release build digest');
    if (platform !== 'mac') {
      throw new Error('Windows release build digest must equal the package layout digest');
    }
  }
  return validatePackageReleaseMetadata({
    schemaVersion: 1,
    kind: 'talking-quill-local-owner-release',
    ...identity,
    releaseBuildDigest: releaseBuildDigest ?? derived,
    packageLayoutDigest: derived,
    update: {
      channel: platform === 'mac' ? `latest-${architecture}-mac` : `latest-${architecture}`,
      payload: platform === 'win' ? 'tqpkg2' : 'zip',
      companion: platform === 'mac' ? 'dmg' : null,
      transactionBinding: 'source-target-package-sha256-v1',
      maintenanceInstaller: platform === 'win' ? 'native-setup' : 'macos-owner-finalizer',
    },
  });
}

export async function writePackageReleaseMetadata(path, options) {
  const metadata = await createPackageReleaseMetadata(options);
  await writeFile(path, `${JSON.stringify(metadata)}\n`, { encoding: 'utf8', mode: 0o600 });
  return metadata;
}

export function windowsUpdatePublicKeyIdentity() {
  const sec1 = readFileSync(WINDOWS_UPDATE_PUBLIC_KEY_PATH, 'utf8').trim();
  if (!/^04[0-9a-f]{128}$/u.test(sec1)) {
    throw new Error('Stable Windows updater public key is missing or malformed');
  }
  return Object.freeze({
    sec1,
    sha256: createHash('sha256').update(Buffer.from(sec1, 'hex')).digest('hex'),
  });
}

export function inspectWindowsUpdaterKey(path, expectedArchitecture) {
  const bytes = readFileSync(path);
  const gatewayMarker = Buffer.from(
    'TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS',
    'ascii',
  );
  if (!bytes.includes(gatewayMarker)) {
    throw new Error('Predecessor artifact is not a Talking Quill gateway');
  }
  const peOffset = bytes.length >= 0x40 ? bytes.readUInt32LE(0x3c) : -1;
  const machine =
    peOffset >= 0 && peOffset + 6 <= bytes.length && bytes.readUInt32LE(peOffset) === 0x0000_4550
      ? bytes.readUInt16LE(peOffset + 4)
      : -1;
  const architecture = machine === 0x8664 ? 'x64' : machine === 0xaa64 ? 'arm64' : null;
  if (
    architecture === null ||
    (expectedArchitecture !== undefined && architecture !== expectedArchitecture)
  ) {
    throw new Error('Predecessor gateway PE architecture does not match release target');
  }
  const marker = Buffer.from('TALKING_QUILL_WINDOWS_UPDATE_PRIMARY_KEY_V1=', 'ascii');
  const offset = bytes.indexOf(marker);
  if (offset < 0 || bytes.indexOf(marker, offset + 1) >= 0) {
    throw new Error('Predecessor gateway must contain one embedded updater-key marker');
  }
  const start = offset + marker.length;
  const sec1 = bytes.subarray(start, start + 130).toString('ascii');
  if (!/^04[0-9a-f]{128}$/u.test(sec1)) {
    throw new Error('Predecessor gateway embedded updater key is malformed');
  }
  return Object.freeze({
    sec1,
    sha256: createHash('sha256').update(Buffer.from(sec1, 'hex')).digest('hex'),
    gatewaySha256: sha256(bytes),
  });
}

export function readPackagePredecessor(environment, platform, architecture) {
  const prefix = 'TALKING_QUILL_PREDECESSOR_';
  const aliases =
    platform === 'mac'
      ? [
          [`${prefix}RELEASE_BUILD`, 'TALKING_QUILL_MACOS_PREDECESSOR_BUILD'],
          [`${prefix}GATEWAY_SHA256`, 'TALKING_QUILL_MACOS_PREDECESSOR_GATEWAY_SHA256'],
          [`${prefix}OWNER_SHA256`, 'TALKING_QUILL_MACOS_PREDECESSOR_OWNER_SHA256'],
        ]
      : [];
  for (const [generic, macos] of aliases) {
    if (
      environment[generic] !== undefined &&
      environment[macos] !== undefined &&
      environment[generic] !== environment[macos]
    ) {
      throw new Error(`Conflicting exact predecessor aliases: ${generic} and ${macos}`);
    }
  }
  const values = {
    version: environment[`${prefix}VERSION`],
    releaseBuildDigest:
      environment[`${prefix}RELEASE_BUILD`] ??
      (platform === 'mac' ? environment.TALKING_QUILL_MACOS_PREDECESSOR_BUILD : undefined),
    gatewaySha256:
      environment[`${prefix}GATEWAY_SHA256`] ??
      (platform === 'mac' ? environment.TALKING_QUILL_MACOS_PREDECESSOR_GATEWAY_SHA256 : undefined),
    ownerSha256:
      environment[`${prefix}OWNER_SHA256`] ??
      (platform === 'mac' ? environment.TALKING_QUILL_MACOS_PREDECESSOR_OWNER_SHA256 : undefined),
  };
  if (Object.values(values).every((value) => value === undefined)) return null;
  if (
    !/^\d+\.\d+\.\d+$/u.test(values.version ?? '') ||
    !HEX_32.test(values.releaseBuildDigest ?? '') ||
    !HEX_32.test(values.gatewaySha256 ?? '') ||
    !HEX_32.test(values.ownerSha256 ?? '')
  ) {
    throw new Error('Package predecessor identity must be complete, versioned, and exact');
  }
  if (platform === 'win') {
    const gatewayPath = environment.TALKING_QUILL_PREDECESSOR_GATEWAY_PATH;
    const ownerPath = environment.TALKING_QUILL_PREDECESSOR_OWNER_PATH;
    if (typeof gatewayPath !== 'string' || gatewayPath.length === 0) {
      throw new Error('Windows predecessor gateway artifact is required');
    }
    if (typeof ownerPath !== 'string' || ownerPath.length === 0) {
      throw new Error('Windows predecessor owner artifact is required');
    }
    const inspected = inspectWindowsUpdaterKey(gatewayPath, architecture);
    if (inspected.gatewaySha256 !== values.gatewaySha256) {
      throw new Error(
        'Windows predecessor gateway artifact hash does not match predecessor policy',
      );
    }
    if (sha256(readFileSync(ownerPath)) !== values.ownerSha256) {
      throw new Error('Windows predecessor owner artifact hash does not match predecessor policy');
    }
    const assertedKey = environment.TALKING_QUILL_PREDECESSOR_UPDATE_PUBLIC_KEY_SHA256;
    if (assertedKey !== undefined && assertedKey !== inspected.sha256) {
      throw new Error('Windows predecessor updater-key assertion does not match embedded bytes');
    }
  }
  return { platform, architecture, ...values };
}

export function validatePackageReleaseMetadata(value) {
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    value.schemaVersion !== 1 ||
    value.kind !== 'talking-quill-local-owner-release'
  ) {
    throw new Error('Package release metadata schema is invalid');
  }
  exactKeys(
    value,
    [
      'schemaVersion',
      'kind',
      'version',
      'platform',
      'architecture',
      'ownerMode',
      'packageMode',
      'sourceCommit',
      'sourceTree',
      'roles',
      'predecessor',
      'outerIdentity',
      'freshInstall',
      'releaseBuildDigest',
      'packageLayoutDigest',
      'update',
    ],
    'package release metadata',
  );
  if (value.freshInstall !== undefined && value.freshInstall !== true) {
    throw new Error('Package fresh-install marker is invalid');
  }
  requireIdentity(value.version, value.platform, value.architecture);
  requireSourceIdentity({ sourceCommit: value.sourceCommit, sourceTree: value.sourceTree });
  requireDigest(value.releaseBuildDigest, 'release build digest');
  requireDigest(value.packageLayoutDigest, 'package layout digest');
  if (value.ownerMode !== 'local-unsigned-enabled') {
    throw new Error('Package release metadata is not owner-enabled');
  }
  if (!['fresh', 'update', 'repair'].includes(value.packageMode)) {
    throw new Error('Package release mode is invalid');
  }
  const layout = ROLE_LAYOUT[value.platform];
  if (!Array.isArray(value.roles) || value.roles.length !== layout.length) {
    throw new Error('Package release role layout is incomplete');
  }
  for (let index = 0; index < layout.length; index += 1) {
    const expected = layout[index];
    const actual = value.roles[index];
    if (actual !== null && typeof actual === 'object' && !Array.isArray(actual)) {
      exactKeys(actual, ['role', 'path', 'sha256', 'suppressionCapable'], 'package role');
    }
    if (
      actual?.role !== expected[0] ||
      actual?.path !== expected[1] ||
      actual?.suppressionCapable !== expected[2] ||
      !HEX_32.test(actual?.sha256 ?? '')
    ) {
      throw new Error('Package release role layout or hash is invalid');
    }
  }
  if (value.roles.filter(({ suppressionCapable }) => suppressionCapable).length !== 1) {
    throw new Error('Package release metadata must name one suppression owner');
  }
  const freshInstall = value.freshInstall === true;
  if (value.platform === 'mac') {
    validateMacosOuterIdentity(value.outerIdentity);
  } else if (value.outerIdentity !== undefined) {
    throw new Error('Windows package metadata cannot declare a macOS outer identity');
  }
  if (value.predecessor !== null && typeof value.predecessor === 'object') {
    exactKeys(
      value.predecessor,
      ['platform', 'architecture', 'version', 'releaseBuildDigest', 'gatewaySha256', 'ownerSha256'],
      'package predecessor',
    );
  }
  if ((value.packageMode === 'fresh') !== freshInstall) {
    throw new Error('Package release mode and fresh-install marker disagree');
  }
  const predecessorRequired = value.packageMode === 'update';
  if (
    (freshInstall && value.predecessor !== null) ||
    (predecessorRequired &&
      (value.predecessor?.platform !== value.platform ||
        value.predecessor?.architecture !== value.architecture ||
        !/^\d+\.\d+\.\d+$/u.test(value.predecessor?.version ?? '') ||
        !HEX_32.test(value.predecessor?.releaseBuildDigest ?? '') ||
        !HEX_32.test(value.predecessor?.gatewaySha256 ?? '') ||
        !HEX_32.test(value.predecessor?.ownerSha256 ?? '') ||
        value.predecessor?.candidateBuildId !== undefined))
  ) {
    throw new Error('Update metadata requires an exact same-architecture predecessor');
  }
  if (value.platform === 'win' && value.releaseBuildDigest !== value.packageLayoutDigest) {
    throw new Error('Windows release build digest must equal the package layout digest');
  }
  if (
    value.packageLayoutDigest !==
    digestCanonicalIdentity({
      version: value.version,
      platform: value.platform,
      architecture: value.architecture,
      ownerMode: value.ownerMode,
      packageMode: value.packageMode,
      sourceCommit: value.sourceCommit,
      sourceTree: value.sourceTree,
      roles: value.roles,
      predecessor: value.predecessor,
      ...(value.platform === 'mac' ? { outerIdentity: value.outerIdentity } : {}),
      ...(freshInstall ? { freshInstall: true } : {}),
    })
  ) {
    throw new Error('Package release layout digest does not match immutable metadata');
  }
  if (value.update !== null && typeof value.update === 'object' && !Array.isArray(value.update)) {
    exactKeys(
      value.update,
      ['channel', 'payload', 'companion', 'transactionBinding', 'maintenanceInstaller'],
      'package updater identity',
    );
  }
  if (
    value.update?.channel !==
      (value.platform === 'mac'
        ? `latest-${value.architecture}-mac`
        : `latest-${value.architecture}`) ||
    value.update?.payload !== (value.platform === 'win' ? 'tqpkg2' : 'zip') ||
    value.update?.companion !== (value.platform === 'mac' ? 'dmg' : null) ||
    value.update?.transactionBinding !== 'source-target-package-sha256-v1' ||
    value.update?.maintenanceInstaller !==
      (value.platform === 'win' ? 'native-setup' : 'macos-owner-finalizer')
  ) {
    throw new Error('Package updater identity is invalid');
  }
  return value;
}

export function verifyMatchingPackageReleaseMetadataBytes(expectedBytes, actualBytes) {
  const expected = Buffer.from(expectedBytes);
  const actual = Buffer.from(actualBytes);
  validatePackageReleaseMetadata(JSON.parse(expected.toString('utf8')));
  validatePackageReleaseMetadata(JSON.parse(actual.toString('utf8')));
  if (!expected.equals(actual)) {
    throw new Error('Final artifact package metadata differs from the inspected unpacked tree');
  }
}

export async function verifySerializedPackageReleaseMetadata(metadataPath, packageRoot, expected) {
  const metadata = validatePackageReleaseMetadata(JSON.parse(await readFile(metadataPath, 'utf8')));
  const sourceIdentity = currentSourceIdentity();
  if (
    metadata.version !== expected.version ||
    metadata.platform !== expected.platform ||
    metadata.architecture !== expected.architecture ||
    metadata.sourceCommit !== sourceIdentity.sourceCommit ||
    metadata.sourceTree !== sourceIdentity.sourceTree
  ) {
    throw new Error(
      'Serialized package release identity does not match the package target or source',
    );
  }
  for (const role of metadata.roles) {
    const path = resolve(packageRoot, role.path);
    if (sha256(await readFile(path)) !== role.sha256) {
      throw new Error(`Serialized package role hash mismatch: ${basename(role.path)}`);
    }
  }
  return metadata;
}

export function createUpdaterReleaseBinding(metadata, packageSha256) {
  validatePackageReleaseMetadata(metadata);
  if (metadata.freshInstall === true || metadata.predecessor === null) {
    throw new Error('Updater publication requires exact predecessor metadata');
  }
  if (metadata.platform === 'mac' && metadata.outerIdentity?.mode !== 'certificate') {
    throw new Error('macOS updater publication requires a certificate-signed outer application');
  }
  requireDigest(packageSha256, 'outer package SHA-256');
  return {
    schemaVersion: 1,
    version: metadata.version,
    platform: metadata.platform,
    architecture: metadata.architecture,
    ownerMode: metadata.ownerMode,
    packageMode: metadata.packageMode,
    sourceCommit: metadata.sourceCommit,
    sourceTree: metadata.sourceTree,
    releaseBuildDigest: metadata.releaseBuildDigest,
    packageLayoutDigest: metadata.packageLayoutDigest,
    packageSha256,
    roles: metadata.roles,
    predecessor: metadata.predecessor,
    ...(metadata.platform === 'mac' ? { outerIdentity: metadata.outerIdentity } : {}),
    channel: metadata.update.channel,
    transactionBinding: metadata.update.transactionBinding,
  };
}

export function authorizeWindowsUpdaterReleaseBinding(
  binding,
  environment = process.env,
  expectedPublicForTest,
) {
  if (binding.platform !== 'win') return binding;
  const privateKeyBase64 = environment.TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64;
  const predecessorArtifact = environment.TALKING_QUILL_PREDECESSOR_GATEWAY_PATH;
  let expectedPublic = expectedPublicForTest;
  if (expectedPublic === undefined) {
    if (typeof predecessorArtifact !== 'string' || predecessorArtifact.length === 0) {
      throw new Error('Exact predecessor gateway artifact is required');
    }
    const inspected = inspectWindowsUpdaterKey(predecessorArtifact, binding.architecture);
    if (inspected.gatewaySha256 !== binding.predecessor.gatewaySha256) {
      throw new Error('Inspected predecessor gateway does not match release binding');
    }
    expectedPublic = inspected.sec1;
  }
  if (typeof privateKeyBase64 !== 'string') {
    throw new Error('Windows updater policy signing key is required');
  }
  const privateKey = createPrivateKey({
    key: Buffer.from(privateKeyBase64, 'base64'),
    format: 'der',
    type: 'pkcs8',
  });
  const actualPublic = createPublicKey(privateKey).export({ format: 'jwk' });
  const sec1 = `04${Buffer.from(actualPublic.x, 'base64url').toString('hex')}${Buffer.from(actualPublic.y, 'base64url').toString('hex')}`;
  if (sec1 !== expectedPublic)
    throw new Error('Windows updater signing key does not match pinned public key');
  const transcript = Buffer.concat([
    Buffer.from('talking-quill/windows-update-authorization/v1\0', 'utf8'),
    Buffer.from(binding.packageSha256, 'hex'),
    Buffer.from(binding.packageLayoutDigest, 'hex'),
  ]);
  return {
    ...binding,
    authorization: {
      scheme: 'p256-sha256-v1',
      verificationKeySha256: createHash('sha256')
        .update(Buffer.from(expectedPublic, 'hex'))
        .digest('hex'),
      signature: sign('sha256', transcript, privateKey).toString('base64'),
    },
  };
}

function digestCanonicalIdentity(identity) {
  const hash = createHash('sha256').update('talking-quill/package-layout/v1\0');
  for (const [name, value] of [
    ['version', identity.version],
    ['platform', identity.platform],
    ['architecture', identity.architecture],
    ['ownerMode', identity.ownerMode],
    ['packageMode', identity.packageMode],
    ['sourceCommit', identity.sourceCommit],
    ['sourceTree', identity.sourceTree],
  ]) {
    frame(hash, name, value);
  }
  for (const role of identity.roles) {
    frame(
      hash,
      'role',
      `${role.role}\0${role.path}\0${role.sha256}\0${String(role.suppressionCapable)}`,
    );
  }
  frame(hash, 'predecessorPresent', String(identity.predecessor !== null));
  if (identity.predecessor !== null) {
    for (const [name, value] of [
      ['predecessorPlatform', identity.predecessor.platform],
      ['predecessorArchitecture', identity.predecessor.architecture],
      ['predecessorVersion', identity.predecessor.version],
      ['predecessorReleaseBuildDigest', identity.predecessor.releaseBuildDigest],
      ['predecessorGatewaySha256', identity.predecessor.gatewaySha256],
      ['predecessorOwnerSha256', identity.predecessor.ownerSha256],
    ])
      frame(hash, name, value);
  }
  if (identity.platform === 'mac')
    frame(hash, 'outerIdentity', JSON.stringify(identity.outerIdentity));
  if (identity.freshInstall === true) frame(hash, 'freshInstall', 'true');
  return hash.digest('hex');
}

function frame(hash, name, value) {
  const bytes = Buffer.from(String(value), 'utf8');
  const header = Buffer.alloc(6);
  header.writeUInt16BE(Buffer.byteLength(name), 0);
  header.writeUInt32BE(bytes.length, 2);
  hash.update(header).update(name).update(bytes);
}
function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
function validateMacosOuterIdentity(value) {
  if (value === null) return;
  if (typeof value === 'object' && !Array.isArray(value)) {
    exactKeys(
      value,
      [
        'mode',
        'leafCertificateSha256',
        'identifier',
        'teamIdentifier',
        'designatedRequirement',
        'designatedRequirementSha256',
      ],
      'macOS outer identity',
    );
  }
  if (
    !['certificate', 'adhoc'].includes(value?.mode) ||
    (value?.mode === 'certificate' && !HEX_32.test(value?.leafCertificateSha256 ?? '')) ||
    (value?.mode === 'adhoc' && value?.leafCertificateSha256 !== null) ||
    !/^[A-Za-z0-9][A-Za-z0-9.-]{0,254}$/u.test(value?.identifier ?? '') ||
    (value?.teamIdentifier !== null && !/^[A-Z0-9]{10}$/u.test(value?.teamIdentifier ?? '')) ||
    typeof value?.designatedRequirement !== 'string' ||
    value.designatedRequirement.length === 0 ||
    Buffer.byteLength(value.designatedRequirement) > 8 * 1024 ||
    value.designatedRequirement.includes('\0') ||
    !HEX_32.test(value?.designatedRequirementSha256 ?? '') ||
    sha256(Buffer.from(value.designatedRequirement, 'utf8')) !==
      value.designatedRequirementSha256 ||
    (value.mode === 'adhoc' && value.teamIdentifier !== null)
  ) {
    throw new Error('macOS outer identity is invalid');
  }
}

function requireIdentity(version, platform, architecture) {
  if (
    !/^\d+\.\d+\.\d+$/u.test(version ?? '') ||
    !Object.hasOwn(ROLE_LAYOUT, platform) ||
    !['x64', 'arm64'].includes(architecture)
  ) {
    throw new Error('Package release version/platform/architecture is invalid');
  }
}
function requireDigest(value, name) {
  if (!HEX_32.test(value ?? '')) throw new Error(`${name} must be lowercase SHA-256`);
}
function requireSourceIdentity(value) {
  if (
    value === null ||
    typeof value !== 'object' ||
    !/^[0-9a-f]{40}$/u.test(value.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(value.sourceTree ?? '')
  ) {
    throw new Error('Package source commit and tree identity are invalid');
  }
}
function exactKeys(value, allowed, name) {
  const expected = new Set(allowed);
  for (const key of Object.keys(value)) {
    if (!expected.has(key)) throw new Error(`${name} contains unknown field: ${key}`);
  }
}
