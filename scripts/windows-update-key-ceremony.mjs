import { createHash, createPublicKey, verify } from 'node:crypto';
import { existsSync, mkdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { createWindowsUpdateNativeSigner } from './windows-update-native-signer.mjs';
import { generateProtectedWindowsUpdateKey } from './windows-update-native-chain.mjs';

const root = resolve(import.meta.dirname, '..');
const version = '0.0.69';
const architecture = 'x64';
const secretDirectory = resolve(root, 'tmp/release-secrets/0.0.69-update-key');
const privateKeyPath = resolve(secretDirectory, 'windows-update-private-key.pkcs8.der');
const publicKeyPath = resolve(root, 'build/windows-update-public-key.sec1');
const evidencePath = resolve(root, 'docs/evidence/windows-update-key-ceremony-0.0.69-x64.json');

export function performWindowsUpdateKeyCeremony() {
  if (process.platform !== 'win32' || process.arch !== 'x64') {
    throw new Error('The Windows updater key ceremony requires native Windows x64');
  }
  if (existsSync(evidencePath) || existsSync(privateKeyPath)) {
    throw new Error('The 0.0.69 Windows updater key ceremony is not fresh');
  }
  const generated = generateProtectedWindowsUpdateKey(privateKeyPath);
  if (!/^04[0-9a-f]{128}$/u.test(generated.publicKeySec1Hex ?? '')) {
    throw new Error('Native key ceremony returned an invalid P-256 public key');
  }
  const native = createWindowsUpdateNativeSigner({ privateKeyPath });
  const probe = Buffer.from('talking-quill/windows-update-key-ceremony/0.0.69/x64/v2\0', 'utf8');
  const signed = native.sign(probe);
  if (signed.publicKeySec1.toString('hex') !== generated.publicKeySec1Hex) {
    throw new Error('Native signer derived a different ceremony public key');
  }
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    signed.publicKeySec1,
  ]);
  if (
    !verify(
      'sha256',
      probe,
      createPublicKey({ key: spki, format: 'der', type: 'spki' }),
      signed.signatureDer,
    )
  ) {
    throw new Error('Native signer ceremony proof did not verify');
  }
  const publicKeySha256 = createHash('sha256').update(signed.publicKeySec1).digest('hex');
  writeFileSync(publicKeyPath, `${generated.publicKeySec1Hex}\n`, 'utf8');
  mkdirSync(resolve(evidencePath, '..'), { recursive: true });
  const identities = native.identities;
  writeFileSync(
    evidencePath,
    `${JSON.stringify(
      {
        schemaVersion: 2,
        purpose: 'talking-quill/windows-update-trust-root-key-ceremony',
        releaseVersion: version,
        architecture,
        performedAtUtc: new Date().toISOString(),
        generator: 'reviewed native P-256 key tool',
        privateKeyEncoding: 'PKCS8 DER',
        publicKeyEncoding: 'SEC1 uncompressed',
        publicKeySha256,
        acl: {
          owner: 'current-user',
          inheritance: 'disabled',
          rights: 'read',
          principals: ['SYSTEM', 'Administrators', 'current-user'],
          nativeHandleValidation: true,
          retainedAncestorValidation: true,
        },
        derivation: {
          implementation: 'reviewed native signer and retained verified-child broker',
          probeSignatureVerified: true,
          sourceCommit: identities.source.sourceCommit,
          sourceTree: identities.source.sourceTree,
          cargoLockSha256: identities.cargoLock.sha256,
          signer: publicIdentity(identities.signer),
          broker: publicIdentity(identities.broker),
          bootstrap: publicIdentity(identities.bootstrap),
        },
        privateMaterialRecorded: false,
      },
      null,
      2,
    )}\n`,
    { flag: 'wx' },
  );
  return Object.freeze({ privateKeyPath, publicKeySha256, evidencePath });
}

function publicIdentity(identity) {
  return { sha256: identity.sha256, bytes: identity.bytes };
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const result = performWindowsUpdateKeyCeremony();
  console.log(JSON.stringify({ result: 'passed', ...result }));
}
