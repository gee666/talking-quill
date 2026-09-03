import { generateKeyPairSync, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
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
  it('checks out and re-verifies the source revision carried by the signed bundle authorization', () => {
    const workflow = readFileSync('.github/workflows/windows-installed-acceptance.yml', 'utf8');
    expect(workflow).toContain('git checkout --detach $sourceRevision');
    expect(workflow).toContain('if ($head -cne $sourceRevision)');
    expect(workflow).toContain('$env:ACCEPTANCE_SOURCE_REVISION = $head');
    expect(workflow).toContain('ref: ${{ github.workflow_sha }}');
    expect(workflow.match(/verify-windows-acceptance-authorization\.mjs/gu)).toHaveLength(3);
    expect(workflow).not.toContain('Expand-Archive');
    expect(workflow).toContain(
      'windows-installed-acceptance-bundle.mjs extract $bundle tmp/windows-installed-acceptance/frozen $env:ACCEPTANCE_ARCHITECTURE - - $env:ACCEPTANCE_BUNDLE_SHA256',
    );
    expect(workflow).toContain('ACCEPTANCE_MANIFEST_SHA256');
    expect(workflow).toContain(
      'windows-installed-acceptance-bundle.mjs verify-tree tmp/windows-installed-acceptance/frozen',
    );
    expect(workflow).toContain('--bundle-root tmp/windows-installed-acceptance/frozen');
    expect(workflow).toContain('--output tmp/windows-installed-acceptance/evidence.json');
    expect(workflow).toContain('frozen/producer-result.json');
    expect(workflow).toContain('--producer-artifact-set-identity');
    expect(workflow).not.toContain('Run the non-mocked protected producer E2E');
    const releaseWorkflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
    expect(releaseWorkflow).toContain('installed-acceptance-x64/producer-result.json');
    expect(releaseWorkflow).toContain('--authorization-sha256');
    expect(releaseWorkflow).toContain('--manifest-sha256');
  });
  it('requires a pinned P-256 signature bound to URL, digest, architecture, and time', () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const manifestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const payload = {
      version: 1,
      purpose: 'talking-quill/windows-installed-acceptance-bundle',
      bundleUrl: 'https://artifacts.example/frozen.zip',
      bundleSha256: '11'.repeat(32),
      producerArtifactSetIdentity: '33'.repeat(32),
      architecture: 'x64',
      sourceRevision: 'ab'.repeat(20),
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
    expect(() =>
      verifyWindowsAcceptanceAuthorization({
        ...input,
        producerArtifactSetIdentity: '44'.repeat(32),
      }),
    ).toThrow('binding is invalid');
  });
});
