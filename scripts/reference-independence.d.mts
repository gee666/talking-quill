export interface BlobEntry {
  readonly path: string;
  readonly blob: string;
}
export interface BlobAllowance extends BlobEntry {
  readonly sourcePaths: readonly string[];
  readonly reason: string;
}
export interface ReferenceAllowance {
  readonly entries: readonly BlobAllowance[];
}
export function pathContainsReferenceComponent(path: string): boolean;
export function compareBlobInventory(
  current: readonly BlobEntry[],
  reference: readonly BlobEntry[],
  allowlist: ReferenceAllowance,
): string[];
