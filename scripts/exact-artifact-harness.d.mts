import type { ChildProcessWithoutNullStreams, SpawnOptions } from 'node:child_process';

export interface ExactArtifact {
  readonly root: string;
  readonly executable: string;
  readonly sha256: string;
  readonly treeSha256: string;
}

export function inspectExactArtifact(options: {
  readonly root: string;
  readonly executable: string;
  readonly expectedSha256?: string;
  readonly expectedTreeSha256?: string;
}): Promise<ExactArtifact>;

export function snapshotExactArtifact(
  artifact: ExactArtifact,
  snapshotParent: string,
): Promise<ExactArtifact>;

export function verifyExactArtifact(artifact: ExactArtifact): Promise<ExactArtifact>;

export function launchExactArtifact(
  artifact: ExactArtifact,
  args: readonly string[],
  options?: Omit<SpawnOptions, 'shell' | 'stdio' | 'windowsHide'>,
): Promise<ChildProcessWithoutNullStreams>;

export function hashFile(path: string): Promise<string>;
