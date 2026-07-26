export interface SecretRule {
  readonly id: string;
  readonly pattern: RegExp;
}
export interface SecretMatch {
  readonly rule: string;
  readonly literal: string;
  readonly index: number;
}

export const SECRET_RULES: readonly SecretRule[];
export const SECRET_SCAN_OVERLAP_BYTES: number;
export function findSecretRuleIds(source: string): readonly string[];
export function findSecretMatches(source: string): readonly SecretMatch[];
