export const ONNX_RUNTIME_PATHS: readonly string[];
export const PROVIDER_LOGO_BASENAMES: readonly string[];
export function discoverFinalArtifactNames(fileNames: readonly string[]): string[];
export function validateAsarEntries(entries: readonly string[]): void;
export function validateSharedReleaseArtifacts(
  artifactNames: readonly string[],
  mode: 'none' | 'nsis' | 'dmg-zip' | string,
  expectedArtifact: {
    readonly version: string;
    readonly platform: 'win' | 'mac' | string;
    readonly arch: 'x64' | 'arm64' | string;
  },
): void;
export function finalArtifactNamesForIdentity(
  artifactNames: readonly string[],
  expectedArtifact: {
    readonly version: string;
    readonly platform: 'win' | 'mac' | string;
    readonly arch: 'x64' | 'arm64' | string;
  },
): string[];
export function validateExpectedFinalArtifacts(
  artifactNames: readonly string[],
  mode: 'none' | 'nsis' | 'dmg-zip' | string,
  expectedArtifact: {
    readonly version: string;
    readonly platform: 'win' | 'mac' | string;
    readonly arch: 'x64' | 'arm64' | string;
  },
): void;
export function validateFinalArtifactInspection(
  produced: number,
  inspected: number,
  strict: boolean,
): void;
export function validatePhysicalEntries(entries: readonly string[]): void;
export function validatePhysicalPackageEntries(
  entries: readonly string[],
  target: 'win' | 'mac',
): void;
export function validateResourceEntries(entries: readonly string[], target: 'win' | 'mac'): void;
export function validateSecretContent(path: string, source: string): void;
export function validateRuntimeContent(path: string, source: string): void;
export function normalizePackagePath(path: string): string;
