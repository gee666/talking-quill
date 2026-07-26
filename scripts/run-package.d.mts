export interface PackagePlan {
  readonly command: string;
  readonly artifactRequirement: 'none' | 'nsis' | 'dmg-zip';
  readonly platform: 'win' | 'mac';
  readonly architecture: 'x64' | 'arm64';
  readonly pnpmArguments: readonly string[];
}

export function createPackagePlan(target: string | undefined): PackagePlan;
