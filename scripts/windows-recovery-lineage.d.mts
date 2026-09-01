export interface WindowsRecoveryArtifact {
  version: string;
  relaunchRecordSchema: number | null;
}

export interface WindowsRecoveryLineage {
  schemaVersion: number;
  currentRelaunchRecordSchema: number;
  unsupportedUnpublishedRelaunchRecordSchemas: number[];
  publishedArtifacts: WindowsRecoveryArtifact[];
}

export function verifyWindowsRecoveryLineage(lineage: WindowsRecoveryLineage): {
  baselineVersion: string;
  schemaVersion: number;
  publishedArtifacts: number;
};

export function verifyWindowsRecoveryLineageFile(configPath: string): Promise<{
  baselineVersion: string;
  schemaVersion: number;
  publishedArtifacts: number;
}>;
