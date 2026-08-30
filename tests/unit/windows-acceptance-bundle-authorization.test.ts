import { generateKeyPairSync, sign } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import { verifyWindowsAcceptanceAuthorization } from '../../scripts/verify-windows-acceptance-authorization.mjs';

function canonical(value: unknown): string {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number') return String(value);
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonical(record[key])}`)
    .join(',')}}`;
}

describe('Windows acceptance bundle authorization', () => {
  it('requires a pinned P-256 signature bound to URL, digest, architecture, and time', () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const manifestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const payload = {
      version: 1,
      purpose: 'talking-quill/windows-installed-acceptance-bundle',
      bundleUrl: 'https://artifacts.example/frozen.zip',
      bundleSha256: '11'.repeat(32),
      architecture: 'x64',
      manifestPublicKeySpkiBase64url: manifestKeys.publicKey
        .export({ format: 'der', type: 'spki' })
        .toString('base64url'),
      issuedAtMs: 900_000,
      notBeforeMs: 1_000_000,
      expiresAtMs: 1_000_000 + 80 * 60_000,
      maxTotalRunMs: 80 * 60_000,
    };
    const envelope = {
      payload,
      signatureBase64url: sign('sha256', Buffer.from(canonical(payload)), {
        key: keys.privateKey,
        dsaEncoding: 'ieee-p1363',
      }).toString('base64url'),
    };
    const input = {
      authorizationBase64url: Buffer.from(canonical(envelope)).toString('base64url'),
      publicKeySpkiBase64url: keys.publicKey
        .export({ format: 'der', type: 'spki' })
        .toString('base64url'),
      bundleUrl: payload.bundleUrl,
      bundleSha256: payload.bundleSha256,
      architecture: 'x64' as const,
      nowMs: 1_030_000,
    };
    expect(verifyWindowsAcceptanceAuthorization(input)).toEqual(payload);
    expect(() =>
      verifyWindowsAcceptanceAuthorization({ ...input, bundleSha256: '22'.repeat(32) }),
    ).toThrow('binding is invalid');
  });
});
