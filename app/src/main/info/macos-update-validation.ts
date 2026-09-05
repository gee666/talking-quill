import { createHash } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { readFile } from 'node:fs/promises';
import { basename, isAbsolute } from 'node:path';

const HEX_32 = /^[0-9a-f]{64}$/u;
const POLICY_BYTES = 328;

export interface DownloadedApplicationUpdate {
  readonly files: readonly string[];
  readonly identity?: {
    readonly schemaVersion: 1;
    readonly version: string;
    readonly platform: 'win' | 'mac';
    readonly architecture: 'x64' | 'arm64';
    readonly ownerMode: 'local-unsigned-enabled';
    readonly packageMode: 'update';
    readonly sourceCommit: string;
    readonly sourceTree: string;
    readonly releaseBuildDigest: string;
    readonly packageLayoutDigest: string;
    readonly packageSha256: string;
    readonly channel: string;
    readonly authorization?: {
      readonly scheme: 'p256-sha256-v1';
      readonly verificationKeySha256: string;
      readonly signature: string;
    };
    readonly roles: readonly {
      readonly role: 'gateway' | 'owner' | 'authority' | 'maintenance' | 'electron';
      readonly path: string;
      readonly sha256: string;
      readonly suppressionCapable: boolean;
    }[];
    readonly outerIdentity?: {
      readonly mode: 'certificate' | 'adhoc';
      readonly leafCertificateSha256: string | null;
      readonly identifier: string;
      readonly teamIdentifier: string | null;
      readonly designatedRequirement: string;
      readonly designatedRequirementSha256: string;
    };
    readonly predecessor: {
      readonly platform: 'win' | 'mac';
      readonly architecture: 'x64' | 'arm64';
      readonly version: string;
      readonly releaseBuildDigest: string;
      readonly gatewaySha256: string;
      readonly ownerSha256: string;
    } | null;
    readonly transactionBinding: 'source-target-package-sha256-v1';
  };
}

export async function sha256File(path: string): Promise<string> {
  const hash = createHash('sha256');
  await new Promise<void>((resolveHash, reject) => {
    const stream = createReadStream(path);
    stream.on('data', (chunk) => hash.update(chunk));
    stream.once('end', resolveHash);
    stream.once('error', reject);
  });
  return hash.digest('hex');
}

export function exactZip(files: readonly string[]): string {
  const zips = files.filter((path) => path.toLowerCase().endsWith('.zip'));
  const candidate = zips[0];
  if (
    zips.length !== 1 ||
    candidate === undefined ||
    !isAbsolute(candidate) ||
    basename(candidate).length === 0
  )
    throw new Error('The macOS update did not provide one exact ZIP candidate');
  return candidate;
}

interface PolicyWire {
  readonly releaseBuildDigest: string;
  readonly gateway: { readonly executableSha256: string };
  readonly owner: { readonly executableSha256: string };
  readonly bridge: { readonly executableSha256: string };
  readonly gatewayReleasePolicy: string;
}

export function validateMacosReplacementIdentity(input: {
  readonly identity: DownloadedApplicationUpdate['identity'];
  readonly archiveSha256: string;
  readonly packageMetadata: unknown;
  readonly source: PolicyWire;
  readonly target: PolicyWire;
  readonly expectedArchitecture: 'x64' | 'arm64' | null;
}): NonNullable<DownloadedApplicationUpdate['identity']> {
  const identity = input.identity;
  if (identity === undefined) {
    throw new Error('Exact macOS updater identity is required');
  }
  const predecessor = identity.predecessor;
  const expectedRoles = [
    {
      role: 'gateway',
      path: 'Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
      sha256: input.target.gateway.executableSha256,
      suppressionCapable: false,
    },
    {
      role: 'owner',
      path: 'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
      sha256: input.target.owner.executableSha256,
      suppressionCapable: true,
    },
    {
      role: 'authority',
      path: 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
      sha256: input.target.bridge.executableSha256,
      suppressionCapable: false,
    },
  ];
  const metadata = input.packageMetadata as Record<string, unknown> | null;
  const metadataUpdate = metadata?.update as Record<string, unknown> | undefined;
  if (
    input.expectedArchitecture === null ||
    identity.platform !== 'mac' ||
    identity.architecture !== input.expectedArchitecture ||
    identity.packageSha256 !== input.archiveSha256 ||
    identity.releaseBuildDigest !== input.target.releaseBuildDigest ||
    identity.roles.length !== expectedRoles.length ||
    identity.roles.some(
      (role, index) => JSON.stringify(role) !== JSON.stringify(expectedRoles[index]),
    ) ||
    predecessor?.platform !== 'mac' ||
    predecessor.architecture !== input.expectedArchitecture ||
    predecessor.releaseBuildDigest !== input.source.releaseBuildDigest ||
    predecessor.gatewaySha256 !== input.source.gateway.executableSha256 ||
    predecessor.ownerSha256 !== input.source.owner.executableSha256 ||
    metadata?.schemaVersion !== 1 ||
    metadata.kind !== 'talking-quill-local-owner-release' ||
    metadata.version !== identity.version ||
    metadata.platform !== identity.platform ||
    metadata.architecture !== identity.architecture ||
    metadata.ownerMode !== identity.ownerMode ||
    metadata.sourceCommit !== identity.sourceCommit ||
    metadata.sourceTree !== identity.sourceTree ||
    metadata.releaseBuildDigest !== identity.releaseBuildDigest ||
    metadata.packageLayoutDigest !== identity.packageLayoutDigest ||
    JSON.stringify(metadata.roles) !== JSON.stringify(identity.roles) ||
    JSON.stringify(metadata.predecessor) !== JSON.stringify(identity.predecessor) ||
    JSON.stringify(metadata.outerIdentity) !== JSON.stringify(identity.outerIdentity) ||
    identity.outerIdentity?.mode !== 'certificate' ||
    metadataUpdate?.channel !== identity.channel ||
    metadataUpdate.payload !== 'zip' ||
    metadataUpdate.transactionBinding !== identity.transactionBinding ||
    metadataUpdate.maintenanceInstaller !== 'macos-owner-finalizer'
  ) {
    throw new Error('Downloaded updater identity does not bind the complete macOS artifact');
  }
  return identity;
}

export async function readPolicy(path: string): Promise<PolicyWire> {
  const wire = JSON.parse(await readFile(path, 'utf8')) as PolicyWire;
  if (
    !HEX_32.test(wire.releaseBuildDigest) ||
    !HEX_32.test(wire.gateway.executableSha256) ||
    !HEX_32.test(wire.owner.executableSha256) ||
    !HEX_32.test(wire.bridge.executableSha256)
  )
    throw new Error('The installed owner policy is malformed');
  return wire;
}

export function assertTransition(source: PolicyWire, target: PolicyWire): void {
  const bytes = Buffer.from(target.gatewayReleasePolicy, 'base64url');
  if (bytes.length !== POLICY_BYTES || bytes.subarray(0, 8).toString('ascii') !== 'TQKOPOL1')
    throw new Error('The target release policy is malformed');
  const hex = (start: number) => bytes.subarray(start, start + 32).toString('hex');
  if (
    hex(16) !== target.releaseBuildDigest ||
    hex(48) !== target.gateway.executableSha256 ||
    hex(80) !== target.owner.executableSha256 ||
    bytes[13] !== 1 ||
    hex(224) !== source.releaseBuildDigest ||
    hex(256) !== source.gateway.executableSha256 ||
    hex(288) !== source.owner.executableSha256
  ) {
    throw new Error('The update does not enroll the exact installed predecessor');
  }
}
