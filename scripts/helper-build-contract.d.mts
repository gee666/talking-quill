export const SOURCE_COMMIT_MARKER: Buffer;
export const SOURCE_TREE_MARKER: Buffer;
export const WINDOWS_TEST_PHYSICAL_MARKER: bigint;
export const WINDOWS_UPDATE_PRIMARY_KEY_MARKER: Buffer;
export const WINDOWS_UPDATE_RECOVERY_LAUNCHER_MARKER: Buffer;
export const RETIRED_WINDOWS_UPDATE_BRIDGE_KEY_MARKER: Buffer;
export const GATEWAY_CANNOT_SUPPRESS_MARKER: Buffer;
export const OWNER_SAFE_DISABLED_MARKER: Buffer;
export const OWNER_LOCAL_ENABLED_MARKER: Buffer;
export const OWNER_TEST_SEAMS_MARKER: Buffer;
export const MACOS_SERVICE_BRIDGE_MARKER: Buffer;
export const LEGACY_NATIVE_MARKERS: readonly Buffer[];

export interface NativeRoleContract {
  readonly name: string;
  readonly role: 'gateway' | 'owner' | 'authority' | 'utility';
  readonly suppressionCapable: boolean;
}

export function nativeRoleLayout(platform: 'win32' | 'darwin'): readonly NativeRoleContract[];
export function verifyOwnerBuildContract(path: string): Promise<void>;
export function verifyHelperBuildContract(
  path: string,
  options: { readonly windows: boolean },
): Promise<void>;
export function verifyMacosServiceBridgeBuildContract(path: string): Promise<void>;
export function verifyCompleteNativeRoleInventory(
  paths: readonly string[],
  assignedRolePaths: readonly string[],
): Promise<void>;
export function verifyNativeSourceIdentity(
  path: string,
  identity: { readonly sourceCommit: string; readonly sourceTree: string },
): Promise<void>;
export function verifyExactlyOneSuppressionCapableExecutable(
  paths: readonly string[],
): Promise<void>;
export function verifyStagedNativeRoleSet(
  directory: string,
  options: {
    readonly platform: 'win32' | 'darwin';
    readonly architecture: 'x64' | 'arm64';
  },
): Promise<void>;
