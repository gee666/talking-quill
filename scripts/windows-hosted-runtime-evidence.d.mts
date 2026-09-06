import type { HostedInstallerBinding } from './windows-hosted-lifecycle-evidence.mjs';
export function verifyHostedRuntimeTree(
  root: string,
  binding: HostedInstallerBinding,
): Promise<{ fileCount: number }>;
export function validateHostedRuntimeEvidence(
  value: Record<string, unknown>,
  binding: HostedInstallerBinding,
): Record<string, unknown>;
export function verifyHostedRuntimeEvidence(options: {
  evidencePath: string;
  installerPath: string;
  provenancePath: string;
  architecture: string;
}): Promise<Record<string, unknown>>;
