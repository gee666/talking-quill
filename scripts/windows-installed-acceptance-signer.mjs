import { createHash, createPrivateKey, createPublicKey, sign } from 'node:crypto';
import { resolve } from 'node:path';
import { stdin, stdout } from 'node:process';
import { fileURLToPath } from 'node:url';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';

const MAX_INPUT_BYTES = 64 * 1024;

export function signAcceptanceInput(input) {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    throw new Error('Signing input is invalid');
  }
  const keyBytes = Buffer.from(input.privateKeyPkcs8Base64 ?? '', 'base64');
  if (
    keyBytes.length === 0 ||
    keyBytes.toString('base64') !== input.privateKeyPkcs8Base64 ||
    keyBytes.length > 512
  ) {
    throw new Error('Signing key encoding is invalid');
  }
  const privateKey = createPrivateKey({ key: keyBytes, format: 'der', type: 'pkcs8' });
  if (
    privateKey.asymmetricKeyType !== 'ec' ||
    privateKey.asymmetricKeyDetails?.namedCurve !== 'prime256v1'
  ) {
    throw new Error('Signing key must be P-256');
  }
  const publicKeySpkiBase64url = Buffer.from(
    createPublicKey(privateKey).export({ format: 'der', type: 'spki' }),
  ).toString('base64url');
  if (input.operation === 'acceptance-envelope') {
    const signatureBase64url = sign('sha256', Buffer.from(canonicalAcceptanceJson(input.payload)), {
      key: privateKey,
      dsaEncoding: 'ieee-p1363',
    }).toString('base64url');
    const envelope = { payload: input.payload, signatureBase64url };
    return Object.freeze({
      encoded: Buffer.from(canonicalAcceptanceJson(envelope)).toString('base64url'),
      publicKeySpkiBase64url,
    });
  }
  if (input.operation === 'windows-update') {
    if (
      !/^[0-9a-f]{64}$/u.test(input.packageSha256 ?? '') ||
      !/^[0-9a-f]{64}$/u.test(input.packageLayoutDigest ?? '')
    ) {
      throw new Error('Windows update signing transcript is invalid');
    }
    const transcript = Buffer.concat([
      Buffer.from('talking-quill/windows-update-authorization/v1\0', 'utf8'),
      Buffer.from(input.packageSha256, 'hex'),
      Buffer.from(input.packageLayoutDigest, 'hex'),
    ]);
    return Object.freeze({
      scheme: 'p256-sha256-v1',
      signature: sign('sha256', transcript, privateKey).toString('base64'),
      verificationKeySha256: createHash('sha256')
        .update(sec1(createPublicKey(privateKey)))
        .digest('hex'),
    });
  }
  throw new Error('Signing operation is invalid');
}

function sec1(publicKey) {
  const jwk = publicKey.export({ format: 'jwk' });
  return Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x, 'base64url'),
    Buffer.from(jwk.y, 'base64url'),
  ]);
}

async function main() {
  const chunks = [];
  let length = 0;
  for await (const chunk of stdin) {
    length += chunk.length;
    if (length > MAX_INPUT_BYTES) throw new Error('Signing input exceeds its bound');
    chunks.push(chunk);
  }
  const bytes = Buffer.concat(chunks);
  const input = JSON.parse(bytes.toString('utf8'));
  const result = signAcceptanceInput(input);
  stdout.write(`${canonicalAcceptanceJson(result)}\n`);
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
