import { createHash } from 'node:crypto';
import { mkdir, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { createWindowsUpdateNativeSigner } from '../../scripts/windows-update-native-signer.mjs';

const root = resolve('tmp/windows-update-native-signer-test');

afterEach(() => rm(root, { recursive: true, force: true }));

describe('Windows update native signer adapter', () => {
  it('passes only the protected key path to the native signer chain', async () => {
    await mkdir(root, { recursive: true });
    const signerPath = resolve(root, 'signer.exe');
    const brokerPath = resolve(root, 'broker.exe');
    const bootstrapPath = resolve(root, 'bootstrap.exe');
    await Promise.all([
      writeFile(signerPath, 'signer'),
      writeFile(brokerPath, 'broker'),
      writeFile(bootstrapPath, 'bootstrap'),
    ]);
    const privateKeyPath = resolve(root, 'protected-secret.pkcs8.der');
    const publicKeySec1Hex = `04${'12'.repeat(64)}`;
    let nativeRequest = '';
    const native = createWindowsUpdateNativeSigner({
      privateKeyPath,
      signerPath,
      brokerPath,
      bootstrapPath,
      signerSha256: createHash('sha256').update('signer').digest('hex'),
      brokerSha256: createHash('sha256').update('broker').digest('hex'),
      bootstrapSha256: createHash('sha256').update('bootstrap').digest('hex'),
      launchProcess: ({ input }: { input: Buffer }) => {
        nativeRequest = input.toString('utf8');
        const request = JSON.parse(nativeRequest) as {
          correlation: string;
          signerSha256: string;
          signerBytes: number;
        };
        return {
          status: 0,
          signal: null,
          stderr: '',
          stdout: JSON.stringify({
            version: 1,
            correlation: request.correlation,
            result: 'passed',
            signerSha256: request.signerSha256,
            signerBytes: request.signerBytes,
            retainedIdentityMatches: true,
            processIdentityMatches: true,
            processHashMatches: true,
            parentIdentityMatches: true,
            creationIdentityMatches: true,
            signatureHex: '34'.repeat(64),
            publicKeySec1Hex,
          }),
        };
      },
    });

    const result = native.sign(Buffer.from('public ceremony probe'));

    expect(nativeRequest).toContain(privateKeyPath.replaceAll('\\', '\\\\'));
    expect(nativeRequest).not.toContain('PRIVATE KEY');
    expect(result.publicKeySec1.toString('hex')).toBe(publicKeySec1Hex);
    expect(result.signatureDer[0]).toBe(0x30);
    expect(() =>
      createWindowsUpdateNativeSigner({
        privateKeyPath,
        signerPath,
        brokerPath,
        bootstrapPath,
        signerSha256: '00'.repeat(32),
      }),
    ).toThrow('does not match its pinned SHA-256');
  });

  it('makes release staging require a protected path and native identities', async () => {
    const source = await import('node:fs/promises').then(({ readFile }) =>
      readFile('scripts/stage-unsigned-release.mjs', 'utf8'),
    );
    expect(source).toContain("valueAfter('--update-private-key')");
    expect(source).toContain('createWindowsUpdateNativeSigner');
    expect(source).not.toContain('WINDOWS_UPDATE_SIGNING_KEY_PKCS8_BASE64');
  });
});
