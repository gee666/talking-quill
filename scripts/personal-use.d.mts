export interface PersonalTarget {
  readonly packageTarget: string;
  readonly platform: 'win' | 'mac';
  readonly architecture: 'x64' | 'arm64';
}

export const WINDOWS_ELEVATION_WRAPPER: string;

export const PERSONAL_TARGETS: Readonly<{
  win: PersonalTarget;
  'mac-x64': PersonalTarget;
  'mac-arm64': PersonalTarget;
}>;

export function createFreshEnvironment(
  configuration: PersonalTarget,
  source?: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv;

export function readMacSigningConfiguration(source?: NodeJS.ProcessEnv): NodeJS.ProcessEnv;
export function requireMatchingArtifactSha256(expected: string, actual: string): void;
export function removeWindowsInstallerStaging(
  staging: string,
  remove?: (path: string, options: { recursive: true; force: true }) => Promise<void>,
  wait?: (milliseconds: number) => Promise<unknown>,
  attempts?: number,
): Promise<void>;
export function withStagedWindowsInstaller<Result>(
  checked: Readonly<{ artifactBytes: Uint8Array; sha256: string }>,
  launch: (stagedInstaller: string, expectedSha256: string) => Result | Promise<Result>,
  stagingParent?: string,
  cleanup?: (staging: string) => Promise<void>,
): Promise<Result>;
export function requireNotRegisteredMacStatus(status: number | null, output: string): void;

export function packagePaths(configuration: PersonalTarget): Readonly<{
  packageRoot: string;
  metadata: string;
  installer: string;
}>;
