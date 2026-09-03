import { createPublicKey, verify } from 'node:crypto';
import { describe, expect, it } from 'vitest';

// RFC 6979 Appendix A.2.5, NIST P-256, SHA-256, message "sample".
const PUBLIC_KEY_SEC1 = Buffer.from(
  '04' +
    '60fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb6' +
    '7903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299',
  'hex',
);
const SIGNATURE_P1363 = Buffer.from(
  'efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716' +
    'f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8',
  'hex',
);

function p256PublicKey(sec1: Buffer) {
  const spkiP256Header = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');
  return createPublicKey({
    key: Buffer.concat([spkiP256Header, sec1]),
    format: 'der',
    type: 'spki',
  });
}

describe('acceptance signer RFC 6979 output', () => {
  it('is accepted by the independent Node crypto verifier as P1363 P-256', () => {
    const key = p256PublicKey(PUBLIC_KEY_SEC1);

    expect(
      verify('sha256', Buffer.from('sample'), { key, dsaEncoding: 'ieee-p1363' }, SIGNATURE_P1363),
    ).toBe(true);
    expect(
      verify('sha256', Buffer.from('changed'), { key, dsaEncoding: 'ieee-p1363' }, SIGNATURE_P1363),
    ).toBe(false);
  });
});
