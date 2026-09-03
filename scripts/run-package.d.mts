export const CANONICAL_PACKAGE_TARGETS: readonly ['win', 'win-arm64'];

export interface PackagePlan {
  readonly command: string;
  readonly artifactRequirement: 'none' | 'native-setup' | 'dmg-zip';
  readonly platform: 'win' | 'mac';
  readonly architecture: 'x64' | 'arm64';
  readonly mode: 'fresh' | 'update';
  readonly directoryTest?: true;
  readonly acceptance?: true;
  readonly pnpmArguments: readonly string[];
}

export function createPackagePlan(target: string | undefined): PackagePlan;
export function createProductionEnvironment(
  plan: PackagePlan,
  sourceEnvironment?: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv;
