import { isAbsolute, resolve } from 'node:path';
import { prepareReviewedWindowsUpdateNativeChain } from './windows-update-native-chain.mjs';
import { signAcceptancePayload } from './windows-installed-acceptance-signer.mjs';

const SPKI_PREFIX = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');

export function createWindowsUpdateNativeSigner(options) {
  if (
    options === null ||
    typeof options !== 'object' ||
    Object.keys(options).join(',') !== 'privateKeyPath' ||
    typeof options.privateKeyPath !== 'string' ||
    !isAbsolute(options.privateKeyPath)
  ) {
    throw new Error('Windows updater signer accepts only a protected private-key path');
  }
  const chain = prepareReviewedWindowsUpdateNativeChain();
  const protectedKeyPath = resolve(options.privateKeyPath);
  return Object.freeze({
    identities: Object.freeze({
      signer: chain.signer,
      broker: chain.broker,
      bootstrap: chain.bootstrap,
      source: chain.source,
      cargoLock: chain.cargoLock,
      provenancePath: chain.provenancePath,
    }),
    sign(payloadBytes) {
      const result = signAcceptancePayload({
        signerPath: chain.signer.path,
        signerSha256: chain.signer.sha256,
        signerBytes: chain.signer.bytes,
        brokerPath: chain.broker.path,
        brokerSha256: chain.broker.sha256,
        brokerBytes: chain.broker.bytes,
        bootstrapIdentity: chain.bootstrap,
        signerSourceCommit: chain.source.sourceCommit,
        signerSourceTree: chain.source.sourceTree,
        privateKeyPath: protectedKeyPath,
        payloadBytes,
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
