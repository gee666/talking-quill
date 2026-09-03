export type ReleasePlatform = 'win' | 'mac';
export type ReleaseArchitecture = 'x64' | 'arm64';
export interface ReleasePredecessor {
  readonly platform: ReleasePlatform;
  readonly architecture: ReleaseArchitecture;
  readonly version: string;
  readonly releaseBuildDigest: string;
  readonly gatewaySha256: string;
  readonly ownerSha256: string;
}
export interface ReleaseRole {
  readonly role: string;
  readonly path: string;
  readonly sha256: string;
  readonly suppressionCapable: boolean;
}
export interface MacosOuterIdentity {
  readonly mode: 'certificate' | 'adhoc';
  readonly leafCertificateSha256: string | null;
  readonly identifier: string;
  readonly teamIdentifier: string | null;
  readonly designatedRequirement: string;
  readonly designatedRequirementSha256: string;
}
export interface PackageReleaseMetadata {
  readonly schemaVersion: 1;
  readonly kind: 'talking-quill-local-owner-release';
  readonly version: string;
  readonly platform: ReleasePlatform;
  readonly architecture: ReleaseArchitecture;
  readonly ownerMode: 'local-unsigned-enabled';
  readonly packageMode: 'fresh' | 'update' | 'repair';
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly roles: readonly ReleaseRole[];
  readonly predecessor: ReleasePredecessor | null;
  readonly outerIdentity?: MacosOuterIdentity | null;
  readonly freshInstall?: true;
  readonly releaseBuildDigest: string;
  readonly packageLayoutDigest: string;
  readonly update: {
    readonly channel: string;
    readonly payload: 'tqpkg2' | 'zip';
    readonly companion: 'dmg' | null;
    readonly transactionBinding: 'source-target-package-sha256-v1';
    readonly maintenanceInstaller: 'native-setup' | 'macos-owner-finalizer';
  };
}
export const RELEASE_PACKAGE_METADATA_NAME: 'keyboard-owner-release-v1.json';
export function createPackageReleaseMetadata(options: {
  readonly version: string;
  readonly platform: ReleasePlatform;
  readonly architecture: ReleaseArchitecture;
  readonly packageRoot: string;
  readonly predecessor?: ReleasePredecessor | null;
  readonly releaseBuildDigest?: string;
  readonly sourceIdentity?: { readonly sourceCommit: string; readonly sourceTree: string };
  readonly outerIdentity?: MacosOuterIdentity | null;
  readonly freshInstall?: boolean;
  readonly packageMode?: 'fresh' | 'update' | 'repair';
}): Promise<PackageReleaseMetadata>;
export function writePackageReleaseMetadata(
  path: string,
  options: Parameters<typeof createPackageReleaseMetadata>[0],
): Promise<PackageReleaseMetadata>;
export function windowsUpdatePublicKeyIdentity(): Readonly<{
  sec1: string;
  sha256: string;
}>;
export function inspectWindowsUpdaterKey(
  path: string,
  expectedArchitecture?: ReleaseArchitecture,
): Readonly<{
  sec1: string;
  sha256: string;
  gatewaySha256: string;
}>;
export function readPackagePredecessor(
  environment: Record<string, string | undefined>,
  platform: ReleasePlatform,
  architecture: ReleaseArchitecture,
): ReleasePredecessor | null;
export function validatePackageReleaseMetadata(value: unknown): PackageReleaseMetadata;
export function verifyMatchingPackageReleaseMetadataBytes(
  expectedBytes: Uint8Array,
  actualBytes: Uint8Array,
): void;
export function verifySerializedPackageReleaseMetadata(
  metadataPath: string,
  packageRoot: string,
  expected: {
    readonly version: string;
    readonly platform: ReleasePlatform;
    readonly architecture: ReleaseArchitecture;
  },
): Promise<PackageReleaseMetadata>;
export function createUpdaterReleaseBinding(
  metadata: PackageReleaseMetadata,
  packageSha256: string,
): {
  readonly schemaVersion: 1;
  readonly version: string;
  readonly platform: ReleasePlatform;
  readonly architecture: ReleaseArchitecture;
  readonly packageMode: 'update';
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly releaseBuildDigest: string;
  readonly packageLayoutDigest: string;
  readonly packageSha256: string;
  readonly roles: readonly ReleaseRole[];
  readonly predecessor: ReleasePredecessor | null;
  readonly outerIdentity?: MacosOuterIdentity | null;
  readonly transactionBinding: 'source-target-package-sha256-v1';
};
export function verifyWindowsUpdaterReleaseBinding<T>(
  binding: T,
  expectedPublic?: string,
): T;
export function authorizeWindowsUpdaterReleaseBinding<
  T extends {
    readonly platform: ReleasePlatform;
    readonly packageSha256: string;
    readonly packageLayoutDigest: string;
  },
>(
  binding: T,
  environment?: Record<string, string | undefined>,
  expectedPublicForTest?: string,
): T & {
  readonly authorization: {
    readonly scheme: 'p256-sha256-v1';
    readonly verificationKeySha256: string;
    readonly signature: string;
  };
};
