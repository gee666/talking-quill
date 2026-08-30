export interface WindowsElevationLaunch {
  readonly executable: string;
  readonly arguments: string[];
}

export interface WindowsUpdateCandidateIdentity {
  readonly version: string;
  readonly platform: 'win';
  readonly architecture: 'x64' | 'arm64';
  readonly ownerMode: 'local-unsigned-enabled';
  readonly packageMode: 'update';
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly releaseBuildDigest: string;
  readonly packageSha256: string;
  readonly packageLayoutDigest: string;
  readonly channel: string;
  readonly transactionBinding: 'source-target-package-sha256-v1';
  readonly roles: readonly {
    readonly role: string;
    readonly path: string;
    readonly sha256: string;
    readonly suppressionCapable: boolean;
  }[];
  readonly authorization: {
    readonly scheme: 'p256-sha256-v1';
    readonly verificationKeySha256: string;
    readonly signature: string;
  };
  readonly predecessor: {
    readonly platform: 'win';
    readonly architecture: 'x64' | 'arm64';
    readonly version: string;
    readonly releaseBuildDigest: string;
    readonly gatewaySha256: string;
    readonly ownerSha256: string;
  };
}

export function buildWindowsElevationLaunch(
  _systemRoot: string,
  installedBootstrapPath: string,
  installerPath: string,
  sha256: string,
  candidate: WindowsUpdateCandidateIdentity,
): WindowsElevationLaunch {
  if (
    !/^[0-9a-f]{64}$/u.test(sha256) ||
    installedBootstrapPath.length === 0 ||
    installedBootstrapPath.includes('\0') ||
    installerPath.length === 0 ||
    installerPath.includes('\0') ||
    !isValidCandidate(candidate) ||
    candidate.packageSha256 !== sha256
  ) {
    throw new Error('Invalid downloaded Windows installer identity');
  }
  const request = Buffer.from(
    JSON.stringify({ version: 2, installerPath, sha256, candidate }),
    'utf8',
  ).toString('base64');
  return {
    executable: installedBootstrapPath,
    arguments: [`--windows-update-bootstrap-v2=${request}`],
  };
}

/* Runtime callers can be untyped JavaScript, so recheck literal fields despite the TypeScript contract. */
/* eslint-disable @typescript-eslint/no-unnecessary-condition */
function isValidCandidate(candidate: WindowsUpdateCandidateIdentity): boolean {
  const digest = (value: string): boolean => /^[0-9a-f]{64}$/u.test(value);
  const architecture = candidate.architecture;
  const [gateway, owner] = candidate.roles;
  return (
    candidate.platform === 'win' &&
    (architecture === 'x64' || architecture === 'arm64') &&
    candidate.ownerMode === 'local-unsigned-enabled' &&
    candidate.packageMode === 'update' &&
    /^[0-9a-f]{40}$/u.test(candidate.sourceCommit) &&
    /^[0-9a-f]{40}$/u.test(candidate.sourceTree) &&
    digest(candidate.packageSha256) &&
    candidate.channel === `latest-${architecture}` &&
    candidate.transactionBinding === 'source-target-package-sha256-v1' &&
    /^\d+\.\d+\.\d+$/u.test(candidate.version) &&
    digest(candidate.releaseBuildDigest) &&
    digest(candidate.packageLayoutDigest) &&
    candidate.authorization.scheme === 'p256-sha256-v1' &&
    digest(candidate.authorization.verificationKeySha256) &&
    /^[A-Za-z0-9+/]{8,}={0,2}$/u.test(candidate.authorization.signature) &&
    candidate.roles.length === 2 &&
    gateway?.role === 'gateway' &&
    gateway.path === 'resources/helper/talking-quill-helper.exe' &&
    !gateway.suppressionCapable &&
    digest(gateway.sha256) &&
    owner?.role === 'owner' &&
    owner.path === 'resources/helper/talking-quill-keyboard-owner.exe' &&
    owner.suppressionCapable &&
    digest(owner.sha256) &&
    candidate.predecessor.platform === 'win' &&
    candidate.predecessor.architecture === architecture &&
    /^\d+\.\d+\.\d+$/u.test(candidate.predecessor.version) &&
    digest(candidate.predecessor.releaseBuildDigest) &&
    candidate.predecessor.releaseBuildDigest !== candidate.releaseBuildDigest &&
    digest(candidate.predecessor.gatewaySha256) &&
    digest(candidate.predecessor.ownerSha256)
  );
}
/* eslint-enable @typescript-eslint/no-unnecessary-condition */

export function settleWindowsElevation(
  exitCode: number | null,
  accepted: () => void,
  rejected: () => void,
): void {
  if (exitCode === 0) accepted();
  else rejected();
}
