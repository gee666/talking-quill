// High-confidence ASCII signatures shared by package and full-history inspection.
// Keep expressions bounded so they can be applied to overlapping streamed chunks.
export const SECRET_RULES = Object.freeze([
  Object.freeze({
    id: 'private-key',
    pattern: /-----BEGIN (?:ENCRYPTED |RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----/gu,
  }),
  Object.freeze({
    id: 'pgp-private-key',
    pattern: new RegExp(['-----BEGIN PGP ', 'PRIVATE KEY BLOCK-----'].join(''), 'gu'),
  }),
  Object.freeze({ id: 'aws-access-key', pattern: /\b(?:AKIA|ASIA)[0-9A-Z]{16}\b/gu }),
  Object.freeze({
    id: 'github-token',
    pattern: /\b(?:gh[pousr]_[A-Za-z0-9]{20,256}|github_pat_[A-Za-z0-9_]{20,256})\b/gu,
  }),
  Object.freeze({ id: 'gitlab-token', pattern: /\bglpat-[A-Za-z0-9_-]{20,256}\b/gu }),
  Object.freeze({ id: 'stripe-live-key', pattern: /\bsk_live_[A-Za-z0-9]{20,256}\b/gu }),
  Object.freeze({ id: 'openai-key', pattern: /\bsk-(?:proj-)?[A-Za-z0-9_-]{20,256}\b/gu }),
  Object.freeze({ id: 'google-api-key', pattern: /\bAIza[0-9A-Za-z_-]{35}\b/gu }),
  Object.freeze({ id: 'slack-token', pattern: /\bxox[baprs]-[0-9A-Za-z-]{20,256}\b/gu }),
]);

// Larger than every fixed signature prefix and credential token accepted above.
export const SECRET_SCAN_OVERLAP_BYTES = 512;

export function findSecretRuleIds(source) {
  return Object.freeze([...new Set(findSecretMatches(source).map(({ rule }) => rule))]);
}

export function findSecretMatches(source) {
  const matches = [];
  for (const { id, pattern } of SECRET_RULES) {
    pattern.lastIndex = 0;
    for (const match of source.matchAll(pattern)) {
      matches.push(Object.freeze({ rule: id, literal: match[0], index: match.index }));
    }
    pattern.lastIndex = 0;
  }
  return Object.freeze(matches);
}
