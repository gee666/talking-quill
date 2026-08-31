export const TQPKG2: Readonly<{
  footerSize: number;
  manifestMax: number;
  filesMax: number;
  fileMax: number;
  treeMax: number;
}>;
export interface Tqpkg2File {
  readonly path: string;
  readonly mode: number;
  readonly size: number;
  readonly sha256: string;
  readonly blockOffset: number;
  readonly blockSize: number;
}
export interface Tqpkg2Manifest {
  readonly schemaVersion: 2;
  readonly architecture: 'x64' | 'arm64';
  readonly version: string;
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly packageMode: 'fresh' | 'update' | 'repair';
  readonly predecessor: null | Readonly<Record<string, string>>;
  readonly target: Readonly<{
    releaseBuildDigest: string;
    gatewaySha256: string;
    ownerSha256: string;
  }>;
  readonly faultPhase:
    | null
    | 'staged'
    | 'prepared'
    | 'published'
    | 'registered'
    | 'committed'
    | 'legacyRetiring'
    | 'legacyRetired';
  readonly treeSha256: string;
  readonly files: readonly Tqpkg2File[];
}
export function canonicalJson(value: unknown): string;
export function validateTqpkg2Path(path: string): void;
export function zstdFrameLength(bytes: Buffer): number;
export function tqpkg2TreeDigest(files: readonly Tqpkg2File[]): string;
export function parseTqpkg2(
  bytes: Buffer,
  expectedArchitecture: 'x64' | 'arm64',
  options?: Readonly<{ allowAcceptanceFaults?: boolean }>,
): Readonly<{
  manifest: Tqpkg2Manifest;
  contents: ReadonlyMap<string, Buffer>;
  packageOffset: number;
  packageSize: number;
  manifestSize: number;
}>;
export function bindTqpkg2OwnerManifest(manifest: Tqpkg2Manifest, owner: unknown): void;
