import { generateKeyPairSync, verify } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { signAcceptanceInput } from '../../scripts/windows-installed-acceptance-signer.mjs';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';
import { sanitizedBuildEnvironment } from '../../tmp/build-windows-installed-acceptance-kit.mjs';

const builderSource = readFileSync('tmp/build-windows-installed-acceptance-kit.mjs', 'utf8');

describe('Windows installed-acceptance kit', () => {
  it('keeps every private signing value out of broad build environments', () => {
    expect(
      sanitizedBuildEnvironment({
        PATH: 'tools',
        TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM: 'manifest-secret',
        TALKING_QUILL_WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64: 'update-secret',
        TALKING_QUILL_ACCEPTANCE_REQUEST_PRIVATE_KEY: 'request-secret',
      }),
    ).toEqual({ PATH: 'tools' });
    expect(builderSource).toContain('env: Object.fromEntries(');
    expect(builderSource).toContain('SystemRoot: process.env.SystemRoot');
    expect(builderSource).not.toContain('env: process.env');
  });

  it('signs canonical request envelopes with a P-256 key in the signer module', () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const privateKeyPkcs8Base64 = Buffer.from(
      keys.privateKey.export({ format: 'der', type: 'pkcs8' }),
    ).toString('base64');
    const payload = { purpose: 'test', expiresAtMs: 123 };
    const result = signAcceptanceInput({
      operation: 'acceptance-envelope',
      payload,
      privateKeyPkcs8Base64,
    });
    const envelope = JSON.parse(Buffer.from(result.encoded, 'base64url').toString('utf8')) as {
      payload: unknown;
      signatureBase64url: string;
    };
    expect(envelope.payload).toEqual(payload);
    expect(
      verify(
        'sha256',
        Buffer.from(canonicalAcceptanceJson(payload)),
        { key: keys.publicKey, dsaEncoding: 'ieee-p1363' },
        Buffer.from(envelope.signatureBase64url, 'base64url'),
      ),
    ).toBe(true);
  });

  it('binds the canonical RELEASE, ten ordered faults, source, and deterministic evidence files', () => {
    for (const contract of [
      "descriptor.version !== '0.0.69'",
      "descriptor.packageMode !== 'fresh'",
      "descriptor.variant !== 'canonical'",
      'parseTqpkg2(installerBytes, descriptor.architecture)',
      'ACCEPTANCE_FAULT_PHASES',
      'createInstalledAcceptancePlan(evidenceInput)',
      "classification: 'nonpromotable-installed-acceptance-kit'",
      "resolve(outputRoot, 'evidence-input.json')",
      "resolve(outputRoot, 'bundle-manifest.json')",
    ]) {
      expect(builderSource).toContain(contract);
    }
  });
});
