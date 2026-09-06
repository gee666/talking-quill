export interface ReviewedWindowsUpdateNativeChain {
  readonly signer: { readonly path: string; readonly sha256: string; readonly bytes: number };
  readonly broker: { readonly path: string; readonly sha256: string; readonly bytes: number };
  readonly bootstrap: { readonly path: string; readonly sha256: string; readonly bytes: number };
  readonly keyTool: { readonly path: string; readonly sha256: string; readonly bytes: number };
  readonly source: { readonly sourceCommit: string; readonly sourceTree: string };
  readonly cargoLock: { readonly sha256: string; readonly blob: string };
  readonly provenancePath: string;
}

export interface ProtectedKeyResult {
  readonly result: string;
  readonly publicKeySec1Hex?: string;
}

export function protectSnapshot(path: string): void;
export function verifySnapshotProtection(path: string): void;
export function prepareReviewedWindowsUpdateNativeChain(): ReviewedWindowsUpdateNativeChain;
export function generateProtectedWindowsUpdateKey(keyPath: string): ProtectedKeyResult;
export function validateProtectedWindowsUpdateKey(keyPath: string): ProtectedKeyResult;
export function deleteProtectedWindowsUpdateKey(
  descriptorPath: string,
): ProtectedKeyResult & { readonly keyPath: string };
