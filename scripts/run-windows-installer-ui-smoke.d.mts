export function createWindowsInstallerUiSmokePlan(options?: {
  readonly architecture?: string;
  readonly version?: string;
  readonly installer?: string;
  readonly provenance?: string;
  readonly output?: string;
  readonly variant?: string;
}): Readonly<{
  installer: string;
  provenance: string;
  output: string;
  architecture: 'x64' | 'arm64';
}>;
