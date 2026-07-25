import { createHash } from 'node:crypto';
import { SECRET_RULES, findSecretMatches } from './secret-rules.mjs';

const TOP_LEVEL_KEYS = ['entries', 'schemaVersion'];
const ENTRY_KEYS = [
  'blobOid',
  'blobSha256',
  'classification',
  'contextSha256',
  'count',
  'matchedLiteralSha256',
  'path',
  'rule',
];
const SHA256 = /^[0-9a-f]{64}$/u;
const OID = /^[0-9a-f]{40,64}$/u;
const CLASSIFICATIONS = new Set(['synthetic-fixture', 'reviewed-nonsecret']);
const RULE_IDS = new Set(SECRET_RULES.map(({ id }) => id));

export function validateSecretHistoryAllowlist(value) {
  if (
    !isExactObject(value, TOP_LEVEL_KEYS) ||
    value.schemaVersion !== 2 ||
    !Array.isArray(value.entries)
  )
    throw new Error('Secret history allowlist schema is invalid.');
  const seen = new Set();
  for (const entry of value.entries) {
    if (
      !isExactObject(entry, ENTRY_KEYS) ||
      !OID.test(entry.blobOid) ||
      !SHA256.test(entry.blobSha256) ||
      typeof entry.path !== 'string' ||
      entry.path.length === 0 ||
      entry.path.includes('\0') ||
      !RULE_IDS.has(entry.rule) ||
      !Number.isSafeInteger(entry.count) ||
      entry.count <= 0 ||
      !isHashList(entry.matchedLiteralSha256) ||
      !isHashList(entry.contextSha256) ||
      !CLASSIFICATIONS.has(entry.classification)
    )
      throw new Error('Secret history allowlist entry schema is invalid.');
    const key = `${entry.blobOid}:${entry.rule}`;
    if (seen.has(key)) throw new Error(`Duplicate secret history allowlist entry: ${key}`);
    seen.add(key);
  }
  return value;
}

export function computeSecretEvidence(blob, rule, path) {
  const source = blob.toString('latin1');
  const matches = findSecretMatches(source).filter((match) => match.rule === rule);
  const literalHashes = [];
  const contextHashes = [];
  for (const match of matches) {
    const literal = Buffer.from(match.literal, 'latin1');
    literalHashes.push(hash(literal));
    const start = Math.max(0, match.index - 64);
    const end = Math.min(blob.length, match.index + literal.length + 64);
    contextHashes.push(hash(blob.subarray(start, end)));
  }
  return {
    blobSha256: hash(blob),
    path,
    count: matches.length,
    matchedLiteralSha256: uniqueSorted(literalHashes),
    contextSha256: uniqueSorted(contextHashes),
  };
}

export function evidenceMatches(entry, actual) {
  return (
    entry.blobSha256 === actual.blobSha256 &&
    entry.path === actual.path &&
    entry.count === actual.count &&
    equal(entry.matchedLiteralSha256, actual.matchedLiteralSha256) &&
    equal(entry.contextSha256, actual.contextSha256)
  );
}

function isExactObject(value, keys) {
  return (
    typeof value === 'object' &&
    value !== null &&
    !Array.isArray(value) &&
    equal(Object.keys(value).sort(), [...keys].sort())
  );
}
function isHashList(value) {
  return (
    Array.isArray(value) &&
    value.length > 0 &&
    value.every((item) => typeof item === 'string' && SHA256.test(item)) &&
    equal(value, uniqueSorted(value))
  );
}
function uniqueSorted(values) {
  return [...new Set(values)].sort();
}
function equal(left, right) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}
function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
