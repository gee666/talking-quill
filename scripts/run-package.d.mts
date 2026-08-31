export const CANONICAL_PACKAGE_TARGETS: readonly ['win'];

export interface PackagePlan {
  readonly command: string;
  readonly artifactRequirement: 'none' | 'native-setup' | 'dmg-zip';
  readonly platform: 'win' | 'mac';
  readonly architecture: 'x64' | 'arm64';
  readonly pnpmArguments: readonly string[];
}

export function createPackagePlan(target: string | undefined): PackagePlan;
export function createProductionEnvironment(
  plan: PackagePlan,
  sourceEnvironment?: NodeJS.ProcessEnv,
): NodeJS.ProcessEnv;
