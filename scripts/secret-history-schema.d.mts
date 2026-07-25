export interface SecretHistoryEntry {
  readonly blobOid: string;
  readonly blobSha256: string;
  readonly path: string;
  readonly rule: string;
  readonly count: number;
  readonly matchedLiteralSha256: readonly string[];
  readonly contextSha256: readonly string[];
  readonly classification: 'synthetic-fixture' | 'reviewed-nonsecret';
}
export interface SecretHistoryAllowlist {
  readonly schemaVersion: 2;
  readonly entries: readonly SecretHistoryEntry[];
}
export declare function validateSecretHistoryAllowlist(value: unknown): SecretHistoryAllowlist;
export declare function computeSecretEvidence(
  blob: Buffer,
  rule: string,
  path: string,
): Omit<SecretHistoryEntry, 'blobOid' | 'rule' | 'classification'>;
export declare function evidenceMatches(
  entry: SecretHistoryEntry,
  actual: ReturnType<typeof computeSecretEvidence>,
): boolean;
