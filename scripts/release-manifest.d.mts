export interface ReleaseManifestAsset {
  readonly name: string;
  readonly bytes: number;
  readonly sha256: string;
}
export interface ReleaseManifestProvenance {
  readonly name: string;
  readonly platform: 'win';
  readonly arch: 'x64' | 'arm64';
  readonly mode: 'setup' | 'update';
  readonly sourceTree: string;
  readonly sourceTreeSha256: string;
}
export interface ReleaseManifestBody {
  readonly schemaVersion: 2;
  readonly repository: string;
  readonly tag: string;
  readonly version: string;
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly platform: 'win';
  readonly architecture: 'x64' | 'arm64' | 'x64+arm64';
  readonly promotable: true;
  readonly workflowRunId: string | null;
  readonly generatedAt: string | null;
  readonly provenance: readonly ReleaseManifestProvenance[];
  readonly assets: readonly ReleaseManifestAsset[];
}
export interface ReleaseManifest extends ReleaseManifestBody {
  readonly manifestSha256: string;
}
export function canonicalJson(value: unknown): string;
export function sealReleaseManifest(body: unknown): ReleaseManifest;
export function validateReleaseManifest(value: unknown): ReleaseManifest;
