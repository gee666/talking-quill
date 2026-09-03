export interface AcceptanceBundleExpectation {
  readonly architecture?: 'x64' | 'arm64';
  readonly sourceCommit?: string;
  readonly sourceTree?: string;
  readonly bundleSha256?: string;
  readonly manifestSha256?: string;
}
export interface VerifiedAcceptanceBundle {
  readonly root?: string;
  readonly archivePath?: string;
  readonly manifest: any;
  readonly evidence?: any;
  readonly entries?: readonly Readonly<{ path: string; bytes: Buffer }>[];
}
export function verifyAcceptanceBundleTree(
  rootPath: string,
  expected?: AcceptanceBundleExpectation,
): Promise<VerifiedAcceptanceBundle>;
export function createDeterministicAcceptanceZip(
  rootPath: string,
  outputPath: string,
  expected?: AcceptanceBundleExpectation,
): Promise<
  Readonly<{ path: string; bytes: number; sha256: string; parsed: VerifiedAcceptanceBundle }>
>;
export function verifyAcceptanceBundleArchive(
  archivePath: string,
  expected?: AcceptanceBundleExpectation,
): Promise<VerifiedAcceptanceBundle>;
export function extractVerifiedAcceptanceBundle(
  archivePath: string,
  destination: string,
  expected?: AcceptanceBundleExpectation,
): Promise<VerifiedAcceptanceBundle>;
export function assertNoLinkPath(
  path: string,
  expectation?: { readonly file?: boolean; readonly directory?: boolean },
): Promise<string>;
export function resolveBundlePath(root: string, path: string): string;
