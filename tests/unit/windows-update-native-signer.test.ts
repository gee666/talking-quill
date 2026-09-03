import { resolve } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ signAcceptancePayload: vi.fn() }));

vi.mock('../../scripts/windows-update-native-chain.mjs', () => ({
  prepareReviewedWindowsUpdateNativeChain: () => ({
    signer: { path: 'reviewed/signer.exe', sha256: '11'.repeat(32), bytes: 101 },
    broker: { path: 'reviewed/broker.exe', sha256: '22'.repeat(32), bytes: 202 },
    bootstrap: { path: 'reviewed/bootstrap.exe', sha256: '33'.repeat(32), bytes: 303 },
    source: { sourceCommit: '44'.repeat(20), sourceTree: '55'.repeat(20) },
    cargoLock: { sha256: '66'.repeat(32), blob: '77'.repeat(20) },
    provenancePath: 'reviewed/provenance.json',
  }),
}));

vi.mock('../../scripts/windows-installed-acceptance-signer.mjs', () => ({
  signAcceptancePayload: mocks.signAcceptancePayload,
}));

import { createWindowsUpdateNativeSigner } from '../../scripts/windows-update-native-signer.mjs';

beforeEach(() => {
  mocks.signAcceptancePayload.mockReset();
  mocks.signAcceptancePayload.mockReturnValue({
    signatureBase64url: Buffer.from('34'.repeat(64), 'hex').toString('base64url'),
    publicKeySpkiBase64url: Buffer.concat([
      Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
      Buffer.from(`04${'12'.repeat(64)}`, 'hex'),
    ]).toString('base64url'),
  });
});

describe('Windows update native signer adapter', () => {
  it('accepts only the protected key path and resolves reviewed native identities internally', () => {
    const privateKeyPath = resolve('tmp/protected-secret.pkcs8.der');
    const native = createWindowsUpdateNativeSigner({ privateKeyPath });
    const result = native.sign(Buffer.from('public ceremony probe'));

    expect(mocks.signAcceptancePayload).toHaveBeenCalledWith(
      expect.objectContaining({
        privateKeyPath,
        signerPath: 'reviewed/signer.exe',
        signerSha256: '11'.repeat(32),
        brokerSha256: '22'.repeat(32),
        signerSourceCommit: '44'.repeat(20),
        signerSourceTree: '55'.repeat(20),
      }),
    );
    expect(result.publicKeySec1.toString('hex')).toBe(`04${'12'.repeat(64)}`);
    expect(result.signatureDer[0]).toBe(0x30);
    expect(() =>
      createWindowsUpdateNativeSigner({
        privateKeyPath,
        signerPath: 'attacker.exe',
      } as never),
    ).toThrow('accepts only a protected private-key path');
  });

  it('makes release staging accept no native path or hash options', async () => {
    const source = await import('node:fs/promises').then(({ readFile }) =>
      readFile('scripts/stage-unsigned-release.mjs', 'utf8'),
    );
    expect(source).toContain("options[0] === '--update-private-key'");
    expect(source).toContain('createWindowsUpdateNativeSigner({ privateKeyPath })');
    expect(source).not.toContain("valueAfter('--native-");
    expect(source).not.toContain('WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64');
  });
});
