export interface WindowsRecoveryArtifact {
  version: string;
  predecessorVersion: string | null;
  relaunchRecordSchema: number;
}

export interface WindowsLocalMigrationPolicy {
  sourceVersion: string;
  provenance: string;
  mode: string;
  targetVersion: string;
}

export interface WindowsRecoveryLineage {
  schemaVersion: number;
  currentRelaunchRecordSchema: number;
  unsupportedUnpublishedRelaunchRecordSchemas: number[];
  trustRootVersion: string;
  localMigrations: WindowsLocalMigrationPolicy[];
  publishedArtifacts: WindowsRecoveryArtifact[];
}

export interface VerifiedWindowsRecoveryLineage {
  trustRootVersion: string;
  localMigrationSourceVersion: string;
  schemaVersion: number;
  publishedArtifacts: number;
}

export function verifyWindowsRecoveryLineage(
  lineage: WindowsRecoveryLineage,
): VerifiedWindowsRecoveryLineage;
export function verifyWindowsRecoveryLineageFile(
  configPath: string,
): Promise<VerifiedWindowsRecoveryLineage>;
