import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import { lstat, mkdir, open, readFile, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { createUpdaterReleaseBinding } from './release-package-metadata.mjs';
import { signAcceptancePayload } from './windows-installed-acceptance-signer.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { ACCEPTANCE_FAULT_PHASES } from './windows-installed-acceptance-schedule.mjs';
import { readAcceptanceSecretPaths } from './acceptance-secret-path-frame.mjs';
import {
  cleanupAcceptanceNative,
  publishAcceptanceNative,
} from './windows-installed-acceptance-native-publication.mjs';

const root = resolve(import.meta.dirname, '..');
const mode = process.argv[2];
const outputRoot = resolve(valueAfter('--output') ?? '');
const secrets = readAcceptanceSecretPaths();
const sourceCommit = process.env.TALKING_QUILL_RELEASE_COMMIT ?? '';
const sourceTree = process.env.TALKING_QUILL_RELEASE_TREE ?? '';
const nativePublication =
  mode === 'identities'
    ? await publishAcceptanceNative({
        buildId: process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID,
        sourceRoot: resolve(root, 'helper/target/x86_64-pc-windows-msvc/release'),
        outputRoot,
      })
    : undefined;
const nativeRoot = resolve(
  nativePublication?.nativeRoot ?? process.env.TALKING_QUILL_ACCEPTANCE_NATIVE_ROOT ?? '',
);
if (nativeRoot === resolve('')) throw new Error('Protected native publication root is unavailable');
let nativeContext;
try {
  nativeContext = await readNativeContext(nativeRoot);
} catch (error) {
  await failAfterPublication(error);
}
const {
  signerPath,
  signerSha256,
  acceptanceBrokerPath,
  acceptanceBrokerSha256,
  acceptanceBootstrapPath,
  acceptanceBootstrapSha256,
  signWith,
} = nativeContext;

if (mode === 'identities') {
  try {
    const probe = Buffer.from('talking-quill/installed-acceptance/key-identity/v1');
    const manifest = signWith(secrets.manifestPrivateKeyPath, probe);
    const request = signWith(secrets.requestPrivateKeyPath, probe);
    const validation = signWith(secrets.validationPrivateKeyPath, probe);
    const update = signWith(secrets.updatePrivateKeyPath, probe);
    const identities = {
      manifestPublicKeySpkiBase64url: manifest.publicKeySpkiBase64url,
      requestPublicKeySpkiBase64url: request.publicKeySpkiBase64url,
      validationPublicKeySpkiBase64url: validation.publicKeySpkiBase64url,
      updatePublicKeySpkiBase64url: update.publicKeySpkiBase64url,
      signerPath,
      signerSha256,
      acceptanceBrokerPath,
      acceptanceBrokerSha256,
      acceptanceBootstrapPath,
      acceptanceBootstrapSha256,
      sourceCommit,
      sourceTree,
      nativePublication,
    };
    await mkdir(outputRoot, { recursive: true });
    await writeFile(
      resolve(outputRoot, 'signing-identities.json'),
      `${canonicalAcceptanceJson(identities)}\n`,
      { flag: 'wx', mode: 0o600 },
    );
    console.log(canonicalAcceptanceJson(identities));
  } catch (error) {
    await failAfterPublication(error);
  }
} else if (mode === 'manifest') {
  const payloadPath = resolve(outputRoot, 'unsigned-acceptance-manifest.json');
  const payloadBytes = await readFile(payloadPath);
  const payload = JSON.parse(payloadBytes.toString('utf8'));
  if (payloadBytes.toString('utf8') !== `${canonicalAcceptanceJson(payload)}\n`) {
    throw new Error('Unsigned acceptance manifest payload is not canonical');
  }
  const signed = signWith(
    secrets.manifestPrivateKeyPath,
    Buffer.from(canonicalAcceptanceJson(payload)),
  );
  const encoded = Buffer.from(
    canonicalAcceptanceJson({ payload, signatureBase64url: signed.signatureBase64url }),
  ).toString('base64url');
  await writeFile(
    resolve(
      root,
      'tmp/installed-acceptance-build/win-unpacked/resources/windows-installed-acceptance-v1.txt',
    ),
    `${encoded}\n`,
    { flag: 'wx', mode: 0o600 },
  );
  console.log(canonicalAcceptanceJson({ result: 'passed', sha256: hash(Buffer.from(encoded)) }));
} else if (mode === undefined || mode.startsWith('--')) {
  const artifactSet = await sealArtifactSet();
  console.log(canonicalAcceptanceJson(artifactSet));
} else {
  throw new Error('Unknown acceptance artifact sealing mode');
}

async function readNativeContext(nativeRoot) {
  const signerPath = resolve(nativeRoot, 'talking-quill-acceptance-signer.exe');
  const acceptanceBrokerPath = resolve(nativeRoot, 'talking-quill-windows-acceptance-broker.exe');
  const acceptanceBootstrapPath = resolve(nativeRoot, 'talking-quill-helper.exe');
  const signerSha256 = await hashFile(signerPath);
  const signerBytes = (await lstat(signerPath)).size;
  const acceptanceBrokerSha256 = await hashFile(acceptanceBrokerPath);
  const acceptanceBrokerBytes = (await lstat(acceptanceBrokerPath)).size;
  const acceptanceBootstrapSha256 = await hashFile(acceptanceBootstrapPath);
  const acceptanceBootstrapBytes = (await lstat(acceptanceBootstrapPath)).size;
  return {
    signerPath,
    signerSha256,
    acceptanceBrokerPath,
    acceptanceBrokerSha256,
    acceptanceBootstrapPath,
    acceptanceBootstrapSha256,
    signWith: (privateKeyPath, payloadBytes) =>
      signAcceptancePayload({
        signerPath,
        signerSha256,
        signerBytes,
        brokerPath: acceptanceBrokerPath,
        brokerSha256: acceptanceBrokerSha256,
        brokerBytes: acceptanceBrokerBytes,
        bootstrapIdentity: {
          path: acceptanceBootstrapPath,
          sha256: acceptanceBootstrapSha256,
          bytes: acceptanceBootstrapBytes,
        },
        signerSourceCommit: sourceCommit,
        signerSourceTree: sourceTree,
        privateKeyPath,
        payloadBytes,
      }),
  };
}

async function failAfterPublication(primaryError) {
  if (nativePublication === undefined) throw primaryError;
  try {
    await cleanupAcceptanceNative(nativePublication);
  } catch (cleanupError) {
    throw new AggregateError(
      [primaryError, cleanupError],
      'Acceptance identity sealing and native cleanup failed',
    );
  }
  throw primaryError;
}

async function sealArtifactSet() {
  const unpackedRoot = resolve(root, 'tmp/installed-acceptance-build/win-unpacked');
  const metadataPath = resolve(unpackedRoot, 'resources/keyboard-owner-release-v1.json');
  const metadataBytes = await readFile(metadataPath);
  const metadata = JSON.parse(metadataBytes.toString('utf8'));
  const candidatePath = resolve(
    root,
    'tmp/installed-acceptance-build/Talking-Quill-0.0.69-win-x64-update.exe',
  );
  const candidateSha256 = await hashFile(candidatePath);
  const buildManifestPath = resolve(unpackedRoot, 'resources/windows-installed-acceptance-v1.txt');
  const electronPath = resolve(unpackedRoot, 'Talking Quill.exe');
  const appAsarPath = resolve(unpackedRoot, 'resources/app.asar');
  const acceptancePayload = {
    schemaVersion: 1,
    installerSha256: candidateSha256,
    electronSha256: await hashFile(electronPath),
    appAsarSha256: await hashFile(appAsarPath),
    buildManifestSha256: await hashFile(buildManifestPath),
  };
  const unsigned = { ...createUpdaterReleaseBinding(metadata, candidateSha256), acceptancePayload };
  const transcript = Buffer.concat([
    Buffer.from('talking-quill/windows-update-authorization/v1\0'),
    Buffer.from(unsigned.packageSha256, 'hex'),
    Buffer.from(unsigned.packageLayoutDigest, 'hex'),
  ]);
  const signed = signWith(secrets.updatePrivateKeyPath, transcript);
  const publicSec1 = spkiToSec1(signed.publicKeySpkiBase64url);
  const releaseIdentity = {
    ...unsigned,
    authorization: {
      scheme: 'p256-sha256-v1',
      verificationKeySha256: hash(publicSec1),
      signature: p1363ToDer(Buffer.from(signed.signatureBase64url, 'base64url')).toString('base64'),
    },
  };
  const releaseIdentityPath = resolve(outputRoot, 'candidate-release-identity.json');
  await writeFile(releaseIdentityPath, `${JSON.stringify(releaseIdentity, null, 2)}\n`, {
    flag: 'wx',
    mode: 0o600,
  });
  const common = {
    architecture: 'x64',
    unpackedRoot,
    metadataPath,
    metadataSha256: hash(metadataBytes),
  };
  const artifact = async (installerPath, extra = {}) => ({
    ...common,
    installerPath,
    installerSha256: await hashFile(installerPath),
    ...extra,
  });
  const candidate = await artifact(candidatePath, {
    releaseIdentityPath,
    releaseIdentitySha256: await hashFile(releaseIdentityPath),
    electronPath,
    electronSha256: acceptancePayload.electronSha256,
    appAsarPath,
    appAsarSha256: acceptancePayload.appAsarSha256,
    electronRelativePath: 'Talking Quill.exe',
  });
  const repair = await artifact(
    resolve(root, 'tmp/installed-acceptance-build/Talking-Quill-0.0.69-win-x64-repair.exe'),
  );
  const faults = {};
  for (const phase of ACCEPTANCE_FAULT_PHASES) {
    const validationEvidencePath = resolve(outputRoot, `fault-validation-${phase}.json`);
    faults[phase] = await artifact(
      resolve(
        root,
        `tmp/installed-acceptance-build/Talking-Quill-0.0.69-win-x64-repair-${phase}.exe`,
      ),
      {
        validationEvidencePath,
        validationEvidenceSha256: await hashFile(validationEvidencePath),
      },
    );
  }
  const identities = JSON.parse(
    await readFile(resolve(outputRoot, 'signing-identities.json'), 'utf8'),
  );
  const chainHeadSha256 = await hashFile(
    resolve(outputRoot, `fault-validation-${ACCEPTANCE_FAULT_PHASES.at(-1)}.json`),
  );
  return {
    predecessor: JSON.parse(process.env.TALKING_QUILL_CANONICAL_ARTIFACT_JSON ?? 'null'),
    signerPath: identities.signerPath,
    signerSha256: identities.signerSha256,
    acceptanceBrokerPath: identities.acceptanceBrokerPath,
    acceptanceBrokerSha256: identities.acceptanceBrokerSha256,
    acceptanceBootstrapPath: identities.acceptanceBootstrapPath,
    acceptanceBootstrapSha256: identities.acceptanceBootstrapSha256,
    candidate,
    repair,
    faults,
    buildManifestPath,
    manifestPublicKeySpkiBase64url: identities.manifestPublicKeySpkiBase64url,
    validationPublicKeySpkiBase64url: identities.validationPublicKeySpkiBase64url,
    validationChainHeadSha256: chainHeadSha256,
    syntheticSenderPath: resolve(
      root,
      'helper/target/x86_64-pc-windows-msvc/release/talking-quill-acceptance-synthetic-sender.exe',
    ),
    trustedLauncherPath: resolve(unpackedRoot, 'resources/helper/talking-quill-helper.exe'),
    syntheticSenderArguments: [],
    nativePublication: identities.nativePublication,
  };
}

function spkiToSec1(encoded) {
  const spki = Buffer.from(encoded, 'base64url');
  if (spki.length !== 91) throw new Error('Update signer public key is invalid');
  return spki.subarray(-65);
}

function p1363ToDer(signature) {
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

async function hashFile(path) {
  const before = await lstat(path, { bigint: true });
  if (!before.isFile() || before.isSymbolicLink() || before.nlink !== 1n) {
    throw new Error(`Acceptance artifact is not a stable regular file: ${basename(path)}`);
  }
  const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat({ bigint: true });
    if (opened.dev !== before.dev || opened.ino !== before.ino || opened.nlink !== 1n) {
      throw new Error(`Acceptance artifact identity changed: ${basename(path)}`);
    }
    const bytes = await handle.readFile();
    const after = await lstat(path, { bigint: true });
    const finalOpened = await handle.stat({ bigint: true });
    if (
      after.dev !== opened.dev ||
      after.ino !== opened.ino ||
      finalOpened.dev !== opened.dev ||
      finalOpened.ino !== opened.ino ||
      finalOpened.size !== BigInt(bytes.length) ||
      finalOpened.mtimeNs !== opened.mtimeNs ||
      finalOpened.ctimeNs !== opened.ctimeNs
    ) {
      throw new Error(`Acceptance artifact changed while hashing: ${basename(path)}`);
    }
    return hash(bytes);
  } finally {
    await handle.close();
  }
}

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
