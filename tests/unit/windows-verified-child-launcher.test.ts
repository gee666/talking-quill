import { describe, expect, it } from 'vitest';
import { verifiedChildArguments } from '../../scripts/windows-verified-child-launcher.mjs';

describe('Windows verified-child launcher', () => {
  const bootstrap = { path: 'bootstrap.exe', sha256: '11'.repeat(32), bytes: 101 };
  const child = {
    path: 'broker.exe',
    sha256: '22'.repeat(32),
    bytes: 202,
    arguments: ['--broker-mode'],
  };

  it('binds both pinned executable identities before any child argument', () => {
    expect(verifiedChildArguments(bootstrap, child, 10_000)).toEqual([
      '--windows-installed-acceptance-verified-child-v1',
      bootstrap.sha256,
      '101',
      expect.stringMatching(/broker\.exe$/u),
      child.sha256,
      '202',
      '10000',
      '0',
      '--',
      '--broker-mode',
    ]);
  });

  it('rejects omitted identities and oversized child arguments', () => {
    expect(() => verifiedChildArguments({ ...bootstrap, sha256: '' }, child, 10_000)).toThrow(
      'identity is invalid',
    );
    expect(() =>
      verifiedChildArguments(bootstrap, { ...child, arguments: ['x'.repeat(32_769)] }, 10_000),
    ).toThrow('identity is invalid');
  });
});
