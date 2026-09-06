export interface HostedInstallerBinding {
  architecture: string;
  sourceCommit: string;
  sourceTree: string;
  sourceTreeSha256: string;
  version: string;
  installer: string;
  installerSha256: string;
  bytes: number;
  provenanceDocumentSha256: string;
  files: readonly { path: string; size: number; sha256: string }[];
}
export function hostedInstallerBinding(
  installerPath: string,
  provenancePath: string,
  architecture: string,
): Promise<HostedInstallerBinding>;
export function validateHostedLifecycleEvidence(
  value: Record<string, unknown>,
  binding: HostedInstallerBinding,
): Record<string, unknown>;
export function verifyHostedLifecycleEvidence(options: {
  evidencePath: string;
  installerPath: string;
  provenancePath: string;
  architecture: string;
}): Promise<Record<string, unknown>>;
