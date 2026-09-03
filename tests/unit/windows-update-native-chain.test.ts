import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { parseArguments, validateUpdateKeyPolicy } from '../../scripts/stage-unsigned-release.mjs';

const chain = readFileSync('scripts/windows-update-native-chain.mjs', 'utf8');
const ceremony = readFileSync('scripts/windows-update-key-ceremony.mjs', 'utf8');
const broker = readFileSync('helper/acceptance-signer/src/broker_main.rs', 'utf8');
const keySecurity = readFileSync('helper/acceptance-signer/src/windows_key_security.rs', 'utf8');

describe('reviewed Windows update native chain', () => {
  it('builds fixed locked binaries from clean source and publishes source-bound provenance', () => {
    expect(chain.match(/requireClean: true/gu)).toHaveLength(2);
    expect(chain).toContain('`${commit}:helper/Cargo.lock`');
    expect(chain).toContain("'--locked'");
    expect(chain).toContain("'x86_64-pc-windows-msvc'");
    expect(chain).toContain('TALKING_QUILL_SOURCE_COMMIT=');
    expect(chain).toContain('TALKING_QUILL_SOURCE_TREE=');
    expect(chain).toContain("architecture?.format !== 'pe'");
    expect(chain).toContain("'Talking Quill Update Signing'");
    expect(chain).not.toContain('signerPath: valueAfter');
    expect(chain).not.toContain('brokerPath: valueAfter');
  });

  it('uses native exact key admission before making the signer handle inheritable', () => {
    expect(broker).toContain('open_validated_private_key(&input.private_key_path)');
    expect(broker.indexOf('open_validated_private_key(&input.private_key_path)')).toBeLessThan(
      broker.indexOf('make_inheritable(key.as_raw_handle())'),
    );
    expect(keySecurity).toContain('SE_DACL_PROTECTED');
    expect(keySecurity).toContain('AceCount } != 3');
    expect(keySecurity).toContain('ace.Mask != FILE_GENERIC_READ');
    expect(keySecurity).toContain('info.nNumberOfLinks != 1');
    expect(keySecurity).toContain('FILE_ATTRIBUTE_REPARSE_POINT');
    expect(keySecurity).toContain('retain_ancestors(path)?');
  });

  it('makes fresh staging reject update keys and update staging reject no key', () => {
    const keyPath = resolve('tmp/release-secrets/key.pkcs8.der');
    expect(() => validateUpdateKeyPolicy('win', true, keyPath)).toThrow(
      'Fresh Windows trust-root staging rejects',
    );
    expect(() => validateUpdateKeyPolicy('win', false, undefined)).toThrow(
      'Windows update staging requires',
    );
    expect(() => validateUpdateKeyPolicy('win', true, undefined)).not.toThrow();
    expect(() => validateUpdateKeyPolicy('win', false, keyPath)).not.toThrow();
  });

  it('records a protected cleanup snapshot and deletes without rebuilding Cargo', () => {
    expect(chain).toContain("purpose: 'talking-quill/protected-windows-update-key-cleanup'");
    expect(chain).toContain('fallbackDeleter');
    expect(chain).toContain('verifySnapshotProtection(cleanupRoot)');
    expect(chain).toContain(
      'copyFileSync(chain.keyTool.path, fallbackPath, constants.COPYFILE_EXCL)',
    );
    const deletion = chain.slice(
      chain.indexOf('export function deleteProtectedWindowsUpdateKey'),
      chain.indexOf('function runKeyTool'),
    );
    expect(deletion).toContain('runPreparedKeyTool');
    expect(deletion).toContain('requireRecordedIdentity');
    expect(deletion).not.toContain('prepareReviewedWindowsUpdateNativeChain');
    expect(deletion).not.toContain('runCargo');
    expect(chain).toContain('key-delete --descriptor <absolute-path>');
  });

  it('removes every native identity option from staging and ceremony', () => {
    const keyPath = resolve('tmp/release-secrets/key.pkcs8.der');
    expect(parseArguments(['win', 'x64', '--update-private-key', keyPath])).toEqual({
      platform: 'win',
      arch: 'x64',
      updatePrivateKeyPath: keyPath,
    });
    expect(() => parseArguments(['win', 'x64', '--native-signer', 'attacker.exe'])).toThrow(
      'Usage',
    );
    expect(() => parseArguments(['win', 'x64', '--update-private-key', 'relative.der'])).toThrow(
      'Usage',
    );
    expect(ceremony).not.toContain("valueAfter('--native-");
    expect(ceremony).not.toContain('ceremonyNativeSha256');
    expect(ceremony).toContain('generateProtectedWindowsUpdateKey(privateKeyPath)');
  });
});
