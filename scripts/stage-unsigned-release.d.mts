export interface UpdateFileEvidence {
  readonly size: number;
  readonly sha512: string;
  readonly sha256: string;
}

export interface CanonicalizeUpdateMetadataOptions {
  readonly expectedVersion: string;
  readonly allowedFiles: readonly string[];
  readonly expectedUpdateFile: string;
  readonly evidence: (name: string) => Promise<UpdateFileEvidence>;
  readonly releaseBinding?: {
    readonly schemaVersion: 1;
    readonly version: string;
    readonly packageSha256: string;
    readonly transactionBinding: 'source-target-package-sha256-v1';
    readonly [field: string]: unknown;
  };
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

export function parseArguments(arguments_: readonly string[]): {
  readonly platform: 'win' | 'mac';
  readonly arch: 'x64' | 'arm64';
  readonly updatePrivateKeyPath?: string;
};

export function packageRootForTarget(
  release: string,
  platform: 'win' | 'mac',
  architecture: 'x64' | 'arm64',
): string;

export function canonicalizeUpdateMetadata(
  value: unknown,
  options: CanonicalizeUpdateMetadataOptions,
): Promise<CanonicalUpdateMetadata>;
