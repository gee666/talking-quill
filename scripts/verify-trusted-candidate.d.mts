export const POLICY_PATHS: readonly string[];
export function hashGitPaths(repository: string, paths: readonly string[]): string;
export function parseApprovedFingerprints(value: string): Set<string>;
