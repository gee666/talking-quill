import { createHash } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { signAcceptancePayload } from './windows-installed-acceptance-signer.mjs';

const SPKI_PREFIX = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');

export function createWindowsUpdateNativeSigner({
  privateKeyPath,
  signerPath,
  brokerPath,
  bootstrapPath,
  signerSourceCommit,
  signerSourceTree,
  signerSha256,
  brokerSha256,
  bootstrapSha256,
  launchProcess,
}) {
  const signer = executableIdentity(signerPath, signerSha256);
  const broker = executableIdentity(brokerPath, brokerSha256);
  const bootstrap = executableIdentity(bootstrapPath, bootstrapSha256);
  const protectedKeyPath = resolve(privateKeyPath);
  return Object.freeze({
    identities: Object.freeze({ signer, broker, bootstrap }),
    sign(payloadBytes) {
      const result = signAcceptancePayload({
        signerPath: signer.path,
        signerSha256: signer.sha256,
        signerBytes: signer.bytes,
        brokerPath: broker.path,
        brokerSha256: broker.sha256,
        brokerBytes: broker.bytes,
        bootstrapIdentity: bootstrap,
        signerSourceCommit,
        signerSourceTree,
        privateKeyPath: protectedKeyPath,
        payloadBytes,
        launchProcess,
      });
      const spki = Buffer.from(result.publicKeySpkiBase64url, 'base64url');
      if (
        spki.length !== SPKI_PREFIX.length + 65 ||
        !spki.subarray(0, SPKI_PREFIX.length).equals(SPKI_PREFIX)
      ) {
        throw new Error('Native updater signer returned an invalid P-256 public key');
      }
      return Object.freeze({
        publicKeySec1: spki.subarray(SPKI_PREFIX.length),
        signatureDer: p1363ToDer(Buffer.from(result.signatureBase64url, 'base64url')),
      });
    },
  });
}

function executableIdentity(path, expectedSha256) {
  const absolute = resolve(path);
  const metadata = lstatSync(absolute);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error('Native updater signer identity is not a regular file');
  }
  const bytes = readFileSync(absolute);
  if (bytes.length !== metadata.size) {
    throw new Error('Native updater signer identity changed while hashing');
  }
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  if (expectedSha256 !== undefined && sha256 !== expectedSha256) {
    throw new Error('Native updater signer identity does not match its pinned SHA-256');
  }
  return Object.freeze({ path: absolute, bytes: bytes.length, sha256 });
}

function p1363ToDer(signature) {
  if (signature.length !== 64) throw new Error('Native updater signature is invalid');
  const integer = (value) => {
    let start = 0;
    while (start < value.length - 1 && value[start] === 0) start += 1;
    let bytes = value.subarray(start);
    if ((bytes[0] & 0x80) !== 0) bytes = Buffer.concat([Buffer.from([0]), bytes]);
    return Buffer.concat([Buffer.from([2, bytes.length]), bytes]);
  };
  const r = integer(signature.subarray(0, 32));
  const s = integer(signature.subarray(32));
  return Buffer.concat([Buffer.from([0x30, r.length + s.length]), r, s]);
}
