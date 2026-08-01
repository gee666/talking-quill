export interface UpdateFileEvidence {
  readonly size: number;
  readonly sha512: string;
}

export interface CanonicalizeUpdateMetadataOptions {
  readonly expectedVersion: string;
  readonly allowedFiles: readonly string[];
  readonly expectedUpdateFile: string;
  readonly evidence: (name: string) => Promise<UpdateFileEvidence>;
}

export interface CanonicalUpdateMetadata {
  readonly version: string;
  readonly files: readonly {
    readonly url: string;
    readonly sha512: string;
    readonly size: number;
    readonly blockMapSize?: number;
  }[];
  readonly path: string;
  readonly sha512: string;
  readonly releaseDate?: string;
}

export function canonicalizeUpdateMetadata(
  value: unknown,
  options: CanonicalizeUpdateMetadataOptions,
): Promise<CanonicalUpdateMetadata>;
