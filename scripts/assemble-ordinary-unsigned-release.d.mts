import type { ReleaseManifest } from './release-manifest.mjs';
export function validateStagedRelease(
  value: Record<string, unknown>,
  expected: Record<string, unknown>,
): void;
export function assembleOrdinaryUnsignedRelease(
  preserved: string,
  smoke: string,
  output: string,
): Promise<ReleaseManifest>;
