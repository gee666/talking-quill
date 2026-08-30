import { createHash } from 'node:crypto';
import type { DownloadedApplicationUpdate } from './macos-owner-update-coordinator';

export function parseUnsignedUpdateIdentity(
  value: unknown,
  hostPlatform: NodeJS.Platform,
  architecture: 'x64' | 'arm64',
  version: string,
): NonNullable<DownloadedApplicationUpdate['identity']> {
  const platform = hostPlatform === 'win32' ? 'win' : hostPlatform === 'darwin' ? 'mac' : null;
  const candidate = value as Record<string, unknown> | null;
  const predecessor = candidate?.predecessor as Record<string, unknown> | null | undefined;
  const roles = candidate?.roles as Record<string, unknown>[] | undefined;
  const authorization = candidate?.authorization as Record<string, unknown> | undefined;
  const outerIdentity = candidate?.outerIdentity as Record<string, unknown> | undefined;
  const expectedRoles =
    platform === 'win'
      ? [
          ['gateway', 'resources/helper/talking-quill-helper.exe', false],
          ['owner', 'resources/helper/talking-quill-keyboard-owner.exe', true],
        ]
      : [
          ['gateway', 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper', false],
          [
            'owner',
            'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
            true,
          ],
          [
            'authority',
            'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
            false,
          ],
        ];
  const digest = (field: unknown): field is string =>
    typeof field === 'string' && /^[0-9a-f]{64}$/u.test(field);
  if (
    platform === null ||
    candidate === null ||
    typeof candidate !== 'object' ||
    candidate.schemaVersion !== 1 ||
    candidate.version !== version ||
    candidate.platform !== platform ||
    candidate.architecture !== architecture ||
    candidate.ownerMode !== 'local-unsigned-enabled' ||
    candidate.packageMode !== 'update' ||
    typeof candidate.sourceCommit !== 'string' ||
    !/^[0-9a-f]{40}$/u.test(candidate.sourceCommit) ||
    typeof candidate.sourceTree !== 'string' ||
    !/^[0-9a-f]{40}$/u.test(candidate.sourceTree) ||
    !digest(candidate.releaseBuildDigest) ||
    !digest(candidate.packageLayoutDigest) ||
    !digest(candidate.packageSha256) ||
    candidate.channel !==
      (platform === 'win' ? `latest-${architecture}` : `latest-${architecture}-mac`) ||
    !Array.isArray(roles) ||
    roles.length !== expectedRoles.length ||
    roles.some(
      (role, index) =>
        role.role !== expectedRoles[index]?.[0] ||
        role.path !== expectedRoles[index]?.[1] ||
        role.suppressionCapable !== expectedRoles[index]?.[2] ||
        !digest(role.sha256),
    ) ||
    candidate.transactionBinding !== 'source-target-package-sha256-v1' ||
    (platform === 'mac' &&
      (outerIdentity?.mode !== 'certificate' ||
        !digest(outerIdentity.leafCertificateSha256) ||
        typeof outerIdentity.identifier !== 'string' ||
        !/^[A-Za-z0-9][A-Za-z0-9.-]{0,254}$/u.test(outerIdentity.identifier) ||
        (outerIdentity.teamIdentifier !== null &&
          (typeof outerIdentity.teamIdentifier !== 'string' ||
            !/^[A-Z0-9]{10}$/u.test(outerIdentity.teamIdentifier))) ||
        typeof outerIdentity.designatedRequirement !== 'string' ||
        outerIdentity.designatedRequirement.length === 0 ||
        Buffer.byteLength(outerIdentity.designatedRequirement) > 8 * 1024 ||
        !digest(outerIdentity.designatedRequirementSha256) ||
        createHash('sha256').update(outerIdentity.designatedRequirement).digest('hex') !==
          outerIdentity.designatedRequirementSha256)) ||
    (platform === 'win' &&
      (authorization?.scheme !== 'p256-sha256-v1' ||
        !digest(authorization.verificationKeySha256) ||
        typeof authorization.signature !== 'string' ||
        !/^[A-Za-z0-9+/]{8,}={0,2}$/u.test(authorization.signature))) ||
    predecessor?.platform !== platform ||
    predecessor.architecture !== architecture ||
    typeof predecessor.version !== 'string' ||
    !/^\d+\.\d+\.\d+$/u.test(predecessor.version) ||
    !digest(predecessor.releaseBuildDigest) ||
    !digest(predecessor.gatewaySha256) ||
    !digest(predecessor.ownerSha256) ||
    predecessor.candidateBuildId !== undefined ||
    !/^\d+\.\d+\.\d+$/u.test(version)
  ) {
    throw new Error('Unsigned updater metadata identity is invalid');
  }
  return candidate as unknown as NonNullable<DownloadedApplicationUpdate['identity']>;
}
