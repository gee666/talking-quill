export interface DependencyLicenseRecord {
  name: string;
  version: string;
  license: string;
}

export function assertAllowedDependencyLicenses(
  records: readonly DependencyLicenseRecord[],
  kind: string,
): void;
