import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it, vi } from 'vitest';
import { signAcceptancePayload } from '../../scripts/windows-installed-acceptance-signer.mjs';
import { sanitizedBuildEnvironment } from '../../tmp/build-windows-installed-acceptance-kit.mjs';

const builderSource = readFileSync('tmp/build-windows-installed-acceptance-kit.mjs', 'utf8');
const signerSource = readFileSync('scripts/windows-installed-acceptance-signer.mjs', 'utf8');

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
    expect(builderSource).not.toContain('readRegular(options.requestPrivateKeyPath)');
    expect(builderSource).not.toContain('privateKeyPkcs8Base64');
    expect(signerSource).not.toContain('readFileSync(resolve(privateKeyPath');
    expect(signerSource).not.toContain('createPrivateKey');
  });

  it('passes only the protected key path and payload to the minimal native signer', () => {
    const spawnProcess = vi.fn((...arguments_: unknown[]) => {
      void arguments_;
      return {
        status: 0,
        stderr: '',
        stdout: `${'11'.repeat(64)}\n04${'22'.repeat(64)}\n`,
      };
    });
    const payloadBytes = Buffer.from('fixed canonical payload');
    const signerPath = 'scripts/windows-installed-acceptance-signer.mjs';
    const signerSha256 = createHash('sha256').update(readFileSync(signerPath)).digest('hex');
    const result = signAcceptancePayload({
      signerPath,
      signerSha256,
      privateKeyPath: 'tmp/protected-request-key.der',
      payloadBytes,
      spawnProcess,
    });
    expect(result.signatureBase64url).toBe(
      Buffer.from('11'.repeat(64), 'hex').toString('base64url'),
    );
    const call = spawnProcess.mock.calls[0];
    if (call === undefined) throw new Error('Native signer was not spawned');
    const arguments_ = call[1] as string[];
    const options = call[2] as { input: Buffer; env: Record<string, string> };
    expect(arguments_).toEqual([
      '--private-key',
      expect.stringMatching(/protected-request-key\.der$/u),
    ]);
    expect(options.input).toBe(payloadBytes);
    expect(options.env).not.toHaveProperty('PATH');
    expect(JSON.stringify(options)).not.toContain('private-key-material');
    expect(() =>
      signAcceptancePayload({
        signerPath,
        signerSha256: '00'.repeat(32),
        privateKeyPath: 'tmp/protected-request-key.der',
        payloadBytes,
        spawnProcess,
      }),
    ).toThrow('signer identity is invalid');
    expect(spawnProcess).toHaveBeenCalledOnce();
  });

  it('binds RELEASE buffers, ordered nonces, deterministic ZIP, and self-verification', () => {
    for (const contract of [
      "descriptor.version !== '0.0.69'",
      "descriptor.packageMode !== 'fresh'",
      "descriptor.variant !== 'canonical'",
      'parseTqpkg2(installerBytes, descriptor.architecture)',
      'ACCEPTANCE_FAULT_PHASES',
      'requestNonces',
      'writeFile(releaseCopy, imported.descriptorBytes',
      'writeFile(installerCopy, imported.installerBytes',
      'createDeterministicAcceptanceZip',
      'extractVerifiedAcceptanceBundle',
      'verifyAcceptanceBundleTree(selfCheckRoot',
    ]) {
      expect(builderSource).toContain(contract);
    }
  });
});
