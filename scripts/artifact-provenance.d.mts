export interface ArtifactIdentity {
  readonly version: string;
  readonly platform: 'win' | 'mac';
  readonly arch: 'x64' | 'arm64';
}
export interface ArtifactProvenanceFile {
  readonly role: 'package-file' | 'final-artifact';
  readonly path: string;
  readonly kind: 'file';
  readonly size: number;
  readonly sha256: string;
}
export interface ArtifactProvenanceLink {
  readonly role: 'package-file';
  readonly path: string;
  readonly kind: 'symlink';
  readonly target: string;
}
export interface ArtifactProvenanceManifest {
  readonly schemaVersion: 1;
  readonly sourceCommit: string;
  readonly package: ArtifactIdentity & { readonly root: string };
  readonly entries: readonly (ArtifactProvenanceFile | ArtifactProvenanceLink)[];
}

export const repositoryRoot: string;
export const artifactProvenanceManifestPath: string;
export function writeArtifactProvenanceManifest(
  options: ArtifactIdentity & {
    readonly packageRoot: string;
    readonly artifacts: readonly string[];
  },
): Promise<ArtifactProvenanceManifest>;
export function verifyArtifactProvenanceManifest(): Promise<ArtifactProvenanceManifest>;
export function artifactUploadPaths(manifest: ArtifactProvenanceManifest): readonly string[];
