import {
  createHash,
  createPublicKey,
  generateKeyPairSync,
  type KeyObject,
  sign,
  verify,
} from 'node:crypto';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';
import {
  authorizeWindowsUpdaterReleaseBinding,
  createPackageReleaseMetadata,
  createUpdaterReleaseBinding,
  inspectWindowsUpdaterKey,
  readPackagePredecessor,
  RELEASE_PACKAGE_METADATA_NAME,
  validatePackageReleaseMetadata,
  verifyMatchingPackageReleaseMetadataBytes,
  verifySerializedPackageReleaseMetadata,
  verifyWindowsUpdaterReleaseBinding,
  windowsUpdatePublicKeyIdentity,
} from '../../scripts/release-package-metadata.mjs';

const root = resolve('tmp/release-package-metadata');
const digest = (value: string) => createHash('sha256').update(value).digest('hex');
const nativeSignWith = (privateKey: KeyObject) => {
  const publicJwk = createPublicKey(privateKey).export({ format: 'jwk' });
  const publicKeySec1 = Buffer.from(
    `04${Buffer.from(publicJwk.x ?? '', 'base64url').toString('hex')}${Buffer.from(publicJwk.y ?? '', 'base64url').toString('hex')}`,
    'hex',
  );
  return (payload: Buffer) => ({
    publicKeySec1,
    signatureDer: sign('sha256', payload, privateKey),
  });
};
const predecessorGatewayPath = resolve(root, 'predecessor-gateway.exe');
const predecessorOwnerPath = resolve(root, 'predecessor-owner.exe');
const outerIdentity = {
  mode: 'certificate' as const,
  leafCertificateSha256: digest('outer certificate'),
  identifier: 'com.talkingquill.app',
  teamIdentifier: null,
  designatedRequirement:
    'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"',
  designatedRequirementSha256: digest(
    'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"',
  ),
};
function predecessorGateway(
  architecture: 'x64' | 'arm64' = 'arm64',
  sec1 = windowsUpdatePublicKeyIdentity().sec1,
): Buffer {
  const header = Buffer.alloc(256);
  header.writeUInt16LE(0x5a4d, 0);
  header.writeUInt32LE(0x80, 0x3c);
  header.writeUInt32LE(0x0000_4550, 0x80);
  header.writeUInt16LE(architecture === 'x64' ? 0x8664 : 0xaa64, 0x84);
  return Buffer.concat([
    header,
    Buffer.from('TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS\0'),
    Buffer.from(`TALKING_QUILL_WINDOWS_UPDATE_PRIMARY_KEY_V1=${sec1}\0`),
  ]);
}
const predecessor = (platform: 'win' | 'mac', architecture: 'x64' | 'arm64') => ({
  platform,
  architecture,
  version: '1.2.2',
  releaseBuildDigest: digest('previous build'),
  gatewaySha256: digest('previous gateway'),
  ownerSha256: digest('previous owner'),
});

beforeEach(async () => {
  await rm(root, { recursive: true, force: true });
  await mkdir(root, { recursive: true });
  await writeFile(predecessorGatewayPath, predecessorGateway());
  await writeFile(predecessorOwnerPath, 'previous owner');
  for (const path of [
    'resources/helper/talking-quill-helper.exe',
    'resources/helper/talking-quill-keyboard-owner.exe',
    'resources/helper/talking-quill-update-recovery-launcher.exe',
    'Talking Quill.exe',
    'Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
    'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
    'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
  ]) {
    const target = resolve(root, path);
    await mkdir(resolve(target, '..'), { recursive: true });
    await writeFile(target, path);
  }
});

describe('owner-enabled serialized release package identity', () => {
  it.each([
    ['win', 'x64'],
    ['win', 'arm64'],
    ['mac', 'x64'],
    ['mac', 'arm64'],
  ] as const)('binds one exact native layout for %s/%s', async (platform, architecture) => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform,
      architecture,
      packageRoot: root,
      predecessor: predecessor(platform, architecture),
      ...(platform === 'mac' ? { outerIdentity } : {}),
    });
    expect(metadata).toMatchObject({
      ownerMode: 'local-unsigned-enabled',
      platform,
      architecture,
      predecessor: predecessor(platform, architecture),
      update: {
        channel: platform === 'mac' ? `latest-${architecture}-mac` : `latest-${architecture}`,
      },
    });
    expect(
      metadata.roles.filter((role: { suppressionCapable: boolean }) => role.suppressionCapable),
    ).toHaveLength(1);
    if (platform === 'mac') expect(metadata.outerIdentity).toEqual(outerIdentity);
    if (platform === 'win') {
      expect(metadata.roles.map(({ role }: { role: string }) => role)).toEqual([
        'gateway',
        'owner',
        'recovery-launcher',
      ]);
    }
    const path = resolve(root, RELEASE_PACKAGE_METADATA_NAME);
    await writeFile(path, `${JSON.stringify(metadata)}\n`);
    await expect(
      verifySerializedPackageReleaseMetadata(path, root, {
        version: '1.2.3',
        platform,
        architecture,
      }),
    ).resolves.toEqual(metadata);
  });

  it('preserves native Windows and macOS ARM64 metadata', async () => {
    await expect(
      createPackageReleaseMetadata({
        version: '1.2.3',
        platform: 'win',
        architecture: 'arm64',
        packageRoot: root,
        predecessor: predecessor('win', 'arm64'),
      }),
    ).resolves.toMatchObject({ platform: 'win', architecture: 'arm64' });
    await expect(
      createPackageReleaseMetadata({
        version: '1.2.3',
        platform: 'mac',
        architecture: 'arm64',
        packageRoot: root,
        predecessor: predecessor('mac', 'arm64'),
        outerIdentity,
      }),
    ).resolves.toMatchObject({ platform: 'mac', architecture: 'arm64' });
  });

  it('binds certificate outer identity into macOS updater metadata and rejects ad-hoc publication', async () => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'mac',
      architecture: 'x64',
      packageRoot: root,
      predecessor: predecessor('mac', 'x64'),
      outerIdentity,
    });
    expect(createUpdaterReleaseBinding(metadata, digest('zip'))).toMatchObject({ outerIdentity });
    const adHoc = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'mac',
      architecture: 'x64',
      packageRoot: root,
      predecessor: predecessor('mac', 'x64'),
      outerIdentity: null,
    });
    expect(() => createUpdaterReleaseBinding(adHoc, digest('zip'))).toThrow(
      'certificate-signed outer application',
    );
  });

  it('allows Windows publication without predecessor authority metadata', async () => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'win',
      architecture: 'x64',
      packageRoot: root,
      predecessor: null,
      freshInstall: true,
    });
    expect(metadata).toMatchObject({ freshInstall: true, predecessor: null });
    expect(validatePackageReleaseMetadata(metadata)).toEqual(metadata);
    const serialized = Buffer.from(`${JSON.stringify(metadata)}\n`);
    expect(() => verifyMatchingPackageReleaseMetadataBytes(serialized, serialized)).not.toThrow();
    expect(() =>
      verifyMatchingPackageReleaseMetadataBytes(
        serialized,
        Buffer.from(`${JSON.stringify(metadata, null, 2)}\n`),
      ),
    ).toThrow();
    expect(() => createUpdaterReleaseBinding(metadata, digest('installer'))).toThrow(
      'requires exact predecessor',
    );
    await expect(
      createPackageReleaseMetadata({
        version: '1.2.3',
        platform: 'win',
        architecture: 'x64',
        packageRoot: root,
        predecessor: predecessor('win', 'x64'),
        freshInstall: true,
      }),
    ).rejects.toThrow('cannot declare predecessor');
  });

  it('requires a complete same-architecture immutable predecessor', () => {
    const inspected = inspectWindowsUpdaterKey(predecessorGatewayPath);
    const environment = {
      TALKING_QUILL_PREDECESSOR_VERSION: '1.2.2',
      TALKING_QUILL_PREDECESSOR_RELEASE_BUILD: digest('build'),
      TALKING_QUILL_PREDECESSOR_GATEWAY_SHA256: inspected.gatewaySha256,
      TALKING_QUILL_PREDECESSOR_OWNER_SHA256: digest('previous owner'),
      TALKING_QUILL_PREDECESSOR_GATEWAY_PATH: predecessorGatewayPath,
      TALKING_QUILL_PREDECESSOR_OWNER_PATH: predecessorOwnerPath,
      TALKING_QUILL_PREDECESSOR_UPDATE_PUBLIC_KEY_SHA256: inspected.sha256,
    };
    expect(readPackagePredecessor(environment, 'win', 'arm64')).toEqual({
      platform: 'win',
      architecture: 'arm64',
      version: '1.2.2',
      releaseBuildDigest: digest('build'),
      gatewaySha256: inspected.gatewaySha256,
      ownerSha256: digest('previous owner'),
    });
    expect(() =>
      readPackagePredecessor(
        { ...environment, TALKING_QUILL_PREDECESSOR_UPDATE_PUBLIC_KEY_SHA256: digest('other key') },
        'win',
        'arm64',
      ),
    ).toThrow('does not match embedded bytes');
    expect(() =>
      readPackagePredecessor(
        { ...environment, TALKING_QUILL_PREDECESSOR_OWNER_SHA256: undefined },
        'win',
        'arm64',
      ),
    ).toThrow('complete');
    expect(() =>
      readPackagePredecessor(
        {
          ...environment,
          TALKING_QUILL_PREDECESSOR_RELEASE_BUILD: digest('generic'),
          TALKING_QUILL_MACOS_PREDECESSOR_BUILD: digest('conflict'),
        },
        'mac',
        'arm64',
      ),
    ).toThrow('Conflicting exact predecessor aliases');
  });

  it('roots a Windows updater binding in the configured P-256 release key', async () => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'win',
      architecture: 'x64',
      packageRoot: root,
      predecessor: predecessor('win', 'x64'),
    });
    const binding = createUpdaterReleaseBinding(metadata, digest('installer'));
    const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const jwk = publicKey.export({ format: 'jwk' });
    const publicSec1 = `04${Buffer.from(jwk.x ?? '', 'base64url').toString('hex')}${Buffer.from(jwk.y ?? '', 'base64url').toString('hex')}`;
    const nativeSign = nativeSignWith(privateKey);
    const authorized = authorizeWindowsUpdaterReleaseBinding(binding, {}, publicSec1, nativeSign);
    const transcript = Buffer.concat([
      Buffer.from('talking-quill/windows-update-authorization/v1\0'),
      Buffer.from(binding.packageSha256, 'hex'),
      Buffer.from(binding.packageLayoutDigest, 'hex'),
    ]);
    expect(authorized.authorization.scheme).toBe('p256-sha256-v1');
    expect(authorized.authorization.verificationKeySha256).toBe(
      createHash('sha256').update(Buffer.from(publicSec1, 'hex')).digest('hex'),
    );
    expect(
      verify(
        'sha256',
        transcript,
        publicKey,
        Buffer.from(authorized.authorization.signature, 'base64'),
      ),
    ).toBe(true);
    expect(verifyWindowsUpdaterReleaseBinding(authorized, publicSec1)).toBe(authorized);
    expect(() =>
      verifyWindowsUpdaterReleaseBinding(
        { ...authorized, packageSha256: digest('tampered installer') },
        publicSec1,
      ),
    ).toThrow('signature is invalid');
    expect(() =>
      verifyWindowsUpdaterReleaseBinding(
        {
          ...authorized,
          authorization: { ...authorized.authorization, verificationKeySha256: digest('wrong') },
        },
        publicSec1,
      ),
    ).toThrow('authorization is invalid');
    expect(() =>
      authorizeWindowsUpdaterReleaseBinding(binding, {}, `04${'00'.repeat(64)}`, nativeSign),
    ).toThrow('does not match pinned public key');
    expect(() =>
      authorizeWindowsUpdaterReleaseBinding(binding, {}, publicSec1, () => ({
        publicKeySec1: Buffer.from(publicSec1, 'hex'),
        signatureDer: Buffer.from('3006020101020101', 'hex'),
      })),
    ).toThrow('native signer signature is invalid');
  });

  it('lets the exact predecessor key authorize a successor that embeds only its new primary', async () => {
    const old = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const oldJwk = old.publicKey.export({ format: 'jwk' });
    const oldSec1 = `04${Buffer.from(oldJwk.x ?? '', 'base64url').toString('hex')}${Buffer.from(oldJwk.y ?? '', 'base64url').toString('hex')}`;
    const artifact = resolve(root, 'old-predecessor-gateway.exe');
    const artifactBytes = predecessorGateway('x64', oldSec1);
    await writeFile(artifact, artifactBytes);
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'win',
      architecture: 'x64',
      packageRoot: root,
      predecessor: {
        ...predecessor('win', 'x64'),
        gatewaySha256: createHash('sha256').update(artifactBytes).digest('hex'),
      },
    });
    const binding = createUpdaterReleaseBinding(metadata, digest('bridge installer'));
    const authorized = authorizeWindowsUpdaterReleaseBinding(
      binding,
      { TALKING_QUILL_PREDECESSOR_GATEWAY_PATH: artifact },
      undefined,
      nativeSignWith(old.privateKey),
    );
    expect(authorized.authorization.verificationKeySha256).toBe(
      createHash('sha256').update(Buffer.from(oldSec1, 'hex')).digest('hex'),
    );
    expect(oldSec1).not.toBe(windowsUpdatePublicKeyIdentity().sec1);
  });

  it('rejects authorization from an artifact outside the declared predecessor binding', async () => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'win',
      architecture: 'arm64',
      packageRoot: root,
      predecessor: predecessor('win', 'arm64'),
    });
    const binding = createUpdaterReleaseBinding(metadata, digest('installer'));
    expect(() =>
      authorizeWindowsUpdaterReleaseBinding(binding, {
        TALKING_QUILL_PREDECESSOR_GATEWAY_PATH: predecessorGatewayPath,
      }),
    ).toThrow('does not match release binding');
  });

  it('rejects a predecessor gateway for the wrong PE architecture', () => {
    expect(() => inspectWindowsUpdaterKey(predecessorGatewayPath, 'x64')).toThrow(
      'architecture does not match',
    );
  });

  it('binds Windows predecessor authority, detects role mutation, and binds exact outer bytes', async () => {
    const metadata = await createPackageReleaseMetadata({
      version: '1.2.3',
      platform: 'win',
      architecture: 'x64',
      packageRoot: root,
      predecessor: predecessor('win', 'x64'),
    });
    const binding = createUpdaterReleaseBinding(metadata, digest('installer'));
    expect(binding).toMatchObject({
      version: '1.2.3',
      packageSha256: digest('installer'),
      roles: metadata.roles,
      transactionBinding: 'source-target-package-sha256-v1',
    });
    const changed = JSON.parse(JSON.stringify(metadata)) as {
      roles: { sha256: string }[];
    };
    const gateway = changed.roles[0];
    if (gateway === undefined) throw new Error('Missing gateway fixture');
    gateway.sha256 = digest('substituted gateway');
    expect(() => validatePackageReleaseMetadata(changed)).toThrow('layout digest');
    const duplicateRole = JSON.parse(JSON.stringify(metadata)) as {
      roles: { role: string }[];
    };
    const duplicateTarget = duplicateRole.roles[2];
    if (duplicateTarget === undefined) throw new Error('Missing recovery launcher fixture');
    duplicateTarget.role = 'owner';
    expect(() => validatePackageReleaseMetadata(duplicateRole)).toThrow('role layout or hash');
    const maliciousOwner = JSON.parse(JSON.stringify(metadata)) as {
      roles: { role: string; sha256: string }[];
    };
    const unchangedGateway = maliciousOwner.roles.find(({ role }) => role === 'gateway');
    const owner = maliciousOwner.roles.find(({ role }) => role === 'owner');
    expect(unchangedGateway?.sha256).toBe(metadata.roles[0]?.sha256);
    if (owner === undefined) throw new Error('Missing owner fixture');
    owner.sha256 = digest('malicious owner');
    expect(() => validatePackageReleaseMetadata(maliciousOwner)).toThrow('layout digest');
    await writeFile(
      resolve(root, 'resources/helper/talking-quill-keyboard-owner.exe'),
      'tampered owner',
    );
    const metadataPath = resolve(root, RELEASE_PACKAGE_METADATA_NAME);
    await writeFile(metadataPath, `${JSON.stringify(metadata)}\n`);
    await expect(
      verifySerializedPackageReleaseMetadata(metadataPath, root, {
        version: '1.2.3',
        platform: 'win',
        architecture: 'x64',
      }),
    ).rejects.toThrow('role hash mismatch: talking-quill-keyboard-owner.exe');
    expect(() =>
      validatePackageReleaseMetadata({ ...metadata, unexpected: 'not-canonical' }),
    ).toThrow('unknown field');
    expect(() => validatePackageReleaseMetadata({ ...metadata, freshInstall: false })).toThrow(
      'fresh-install marker',
    );
    await expect(
      createPackageReleaseMetadata({
        version: '1.2.3',
        platform: 'win',
        architecture: 'x64',
        packageRoot: root,
        predecessor: null,
        releaseBuildDigest: digest('substituted release build'),
      }),
    ).rejects.toThrow('must equal the package layout');
  });
});
