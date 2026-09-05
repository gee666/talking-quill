import { createPublicKey, verify, type KeyObject } from 'node:crypto';

const MAX_ENVELOPE_BYTES = 16 * 1024;

export function canonicalAcceptanceJson(value: unknown): string {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value))
      throw new Error('Canonical acceptance numbers must be integers');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalAcceptanceJson).join(',')}]`;
  if (typeof value !== 'object') throw new Error('Unsupported canonical acceptance value');
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalAcceptanceJson(record[key])}`)
    .join(',')}}`;
}

export function encodeCanonicalAcceptanceEnvelope(value: unknown): string {
  return Buffer.from(canonicalAcceptanceJson(value), 'utf8').toString('base64url');
}

export function acceptancePayloadBytes(payload: unknown): Buffer {
  return Buffer.from(canonicalAcceptanceJson(payload), 'utf8');
}

export function decodeCanonicalEnvelope<T>(
  encoded: string,
  parse: (value: unknown) => T,
  label: string,
): T {
  if (!/^[A-Za-z0-9_-]+$/u.test(encoded) || encoded.length > MAX_ENVELOPE_BYTES * 2) {
    throw new Error(`Encoded ${label} is invalid`);
  }
  const bytes = Buffer.from(encoded, 'base64url');
  if (
    bytes.length === 0 ||
    bytes.length > MAX_ENVELOPE_BYTES ||
    bytes.toString('base64url') !== encoded
  ) {
    throw new Error(`Encoded ${label} is invalid`);
  }
  let value: unknown;
  try {
    value = JSON.parse(bytes.toString('utf8')) as unknown;
  } catch {
    throw new Error(`Encoded ${label} is not JSON`);
  }
  const parsed = parse(value);
  if (encodeCanonicalAcceptanceEnvelope(parsed) !== encoded) {
    throw new Error(`Encoded ${label} is not canonical`);
  }
  return parsed;
}

export function readP256PublicKey(encoded: string, label: string): KeyObject {
  if (!/^[A-Za-z0-9_-]+$/u.test(encoded) || encoded.length > 256) {
    throw new Error(`${label} is invalid`);
  }
  const der = Buffer.from(encoded, 'base64url');
  if (der.toString('base64url') !== encoded) throw new Error(`${label} is invalid`);
  let key: KeyObject;
  try {
    key = createPublicKey({ key: der, format: 'der', type: 'spki' });
  } catch {
    throw new Error(`${label} is invalid`);
  }
  if (key.asymmetricKeyType !== 'ec' || key.asymmetricKeyDetails?.namedCurve !== 'prime256v1') {
    throw new Error(`${label} must be a P-256 public key`);
  }
  if (!Buffer.from(key.export({ format: 'der', type: 'spki' })).equals(der)) {
    throw new Error(`${label} is not canonical DER`);
  }
  return key;
}

export function verifyPayload(payload: unknown, encodedSignature: string, key: KeyObject): boolean {
  const signature = Buffer.from(encodedSignature, 'base64url');
  return (
    signature.length === 64 &&
    signature.toString('base64url') === encodedSignature &&
    verify('sha256', acceptancePayloadBytes(payload), { key, dsaEncoding: 'ieee-p1363' }, signature)
  );
}
