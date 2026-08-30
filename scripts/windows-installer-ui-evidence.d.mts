export const WINDOWS_INSTALLER_UI_CANCELLATION_EXIT_CODE: 0;
export interface WindowsInstallerUiEvidenceExpectation {
  readonly installer: string;
  readonly architecture: 'x64' | 'arm64';
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly sourceTreeSha256: string;
  readonly installerSha256: string;
  readonly provenanceDocumentSha256: string;
  readonly bytes: number;
}
export function validateWindowsInstallerUiEvidence(
  value: unknown,
  expected: WindowsInstallerUiEvidenceExpectation,
): unknown;
export function verifyWindowsInstallerUiEvidence(options: {
  readonly evidencePath: string;
  readonly installerPath: string;
  readonly provenancePath: string;
  readonly architecture: 'x64' | 'arm64';
}): Promise<unknown>;
