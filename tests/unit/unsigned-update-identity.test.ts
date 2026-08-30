import { describe, expect, it } from 'vitest';
import { parseUnsignedUpdateIdentity } from '../../app/src/main/info/unsigned-update-identity';

const digest = (character: string) => character.repeat(64);
const identity = {
  schemaVersion: 1,
  version: '1.2.3',
  platform: 'mac',
  architecture: 'arm64',
  ownerMode: 'local-unsigned-enabled',
  packageMode: 'update',
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'b'.repeat(40),
  releaseBuildDigest: digest('a'),
  packageLayoutDigest: digest('b'),
  packageSha256: digest('c'),
  channel: 'latest-arm64-mac',
  roles: [
    {
      role: 'gateway',
      path: 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
      sha256: digest('1'),
      suppressionCapable: false,
    },
    {
      role: 'owner',
      path: 'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
      sha256: digest('2'),
      suppressionCapable: true,
    },
    {
      role: 'authority',
      path: 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
      sha256: digest('3'),
      suppressionCapable: false,
    },
  ],
  outerIdentity: {
    mode: 'certificate',
    leafCertificateSha256: digest('9'),
    identifier: 'com.talkingquill.app',
    teamIdentifier: null,
    designatedRequirement:
      'identifier "com.talkingquill.app" and anchor trusted and certificate leaf = H"0123"',
    designatedRequirementSha256: 'cb5cdda6add7abbf93ef06fda094aff9a64d487cbddc23e7c948dd8ec0b08933',
  },
  predecessor: {
    platform: 'mac',
    architecture: 'arm64',
    version: '1.2.2',
    releaseBuildDigest: digest('d'),
    gatewaySha256: digest('e'),
    ownerSha256: digest('f'),
  },
  transactionBinding: 'source-target-package-sha256-v1',
} as const;

describe('runtime unsigned updater identity', () => {
  it('accepts an exact architecture and predecessor-bound macOS payload identity', () => {
    expect(parseUnsignedUpdateIdentity(identity, 'darwin', 'arm64', '1.2.3')).toEqual(identity);
  });

  it('accepts an exact same-version refresh when the predecessor build and role hashes differ', () => {
    expect(
      parseUnsignedUpdateIdentity(
        { ...identity, predecessor: { ...identity.predecessor, version: identity.version } },
        'darwin',
        'arm64',
        identity.version,
      ),
    ).toBeDefined();
  });

  it('accepts predecessor-bound Windows x64 and ARM64 contracts', () => {
    const windowsIdentity = {
      ...identity,
      platform: 'win',
      architecture: 'x64',
      channel: 'latest-x64',
      outerIdentity: undefined,
      authorization: {
        scheme: 'p256-sha256-v1',
        verificationKeySha256: '66'.repeat(32),
        signature: 'MEUCIQfixture==',
      },
      predecessor: {
        ...identity.predecessor,
        platform: 'win',
        architecture: 'x64',
      },
      roles: [
        {
          role: 'gateway',
          path: 'resources/helper/talking-quill-helper.exe',
          sha256: digest('1'),
          suppressionCapable: false,
        },
        {
          role: 'owner',
          path: 'resources/helper/talking-quill-keyboard-owner.exe',
          sha256: digest('2'),
          suppressionCapable: true,
        },
      ],
    } as const;
    expect(parseUnsignedUpdateIdentity(windowsIdentity, 'win32', 'x64', '1.2.3')).toEqual(
      windowsIdentity,
    );
    const arm64Identity = {
      ...windowsIdentity,
      architecture: 'arm64',
      channel: 'latest-arm64',
      predecessor: { ...windowsIdentity.predecessor, architecture: 'arm64' },
    } as const;
    expect(parseUnsignedUpdateIdentity(arm64Identity, 'win32', 'arm64', '1.2.3')).toEqual(
      arm64Identity,
    );
  });

  it.each([
    { ...identity, version: undefined },
    { ...identity, version: '1.2.4' },
    { ...identity, architecture: 'x64' },
    { ...identity, predecessor: null },
    { ...identity, packageSha256: digest('A') },
    {
      ...identity,
      roles: identity.roles.map((role, index) =>
        index === 1 ? { ...role, sha256: digest('0').slice(1) } : role,
      ),
    },
    { ...identity, predecessor: { ...identity.predecessor, platform: 'win' } },
    { ...identity, predecessor: { ...identity.predecessor, ownerSha256: digest('0').slice(1) } },
  ])('rejects substituted/cross-target metadata %#', (changed) => {
    expect(() => parseUnsignedUpdateIdentity(changed, 'darwin', 'arm64', '1.2.3')).toThrow(
      'identity is invalid',
    );
  });
});
