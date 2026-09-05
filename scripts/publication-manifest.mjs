import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign as signBytes,
  verify as verifyBytes,
} from 'node:crypto';
import { lstat, readFile, readdir, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalJson } from './release-manifest.mjs';

const DOMAIN = Buffer.from('TalkingQuill/release-publication-manifest/v1\0');
const HEX = /^[0-9a-f]{64}$/u;
const SOURCE = /^[0-9a-f]{40}$/u;
const RUN = /^[1-9][0-9]*$/u;
const safeName = (value) =>
  typeof value === 'string' &&
  value.length > 0 &&
  value.length <= 255 &&
  !/[\\/\p{C}]/u.test(value) &&
  value !== '.' &&
  value !== '..';
const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');

function exactObject(value, keys, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error(`${label} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index]))
    throw new Error(`${label} has an unexpected schema`);
}

function publicSec1(key) {
  const jwk = createPublicKey(key).export({ format: 'jwk' });
  return Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x, 'base64url'),
    Buffer.from(jwk.y, 'base64url'),
  ]);
}

async function inventory(directory, excluded = new Set()) {
  const assets = [];
  const objects = new Map();
  for (const name of (await readdir(directory)).sort()) {
    if (excluded.has(name)) continue;
    if (!safeName(name)) throw new Error(`Publication asset name is unsafe: ${String(name)}`);
    const path = resolve(directory, name);
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink())
      throw new Error(`Publication asset is not a regular file: ${name}`);
    const digest = sha256(await readFile(path));
    const prior = objects.get(digest);
    if (prior !== undefined && prior.bytes !== metadata.size)
      throw new Error('Content-addressed publication object size conflicts');
    objects.set(digest, {
      sha256: digest,
      bytes: metadata.size,
      objectName: `sha256-${digest}`,
    });
    assets.push({ name, objectSha256: digest });
  }
  return {
    objects: [...objects.values()].sort((left, right) => left.sha256.localeCompare(right.sha256)),
    assets,
  };
}

function validatePayload(payload) {
  exactObject(
    payload,
    [
      'schemaVersion',
      'repository',
      'tag',
      'sequence',
      'workflowRunId',
      'sourceCommit',
      'sourceTree',
      'objects',
      'assets',
      'promotionEvidenceSha256',
      'releaseManifestSha256',
    ],
    'publication payload',
  );
  if (
    payload.schemaVersion !== 1 ||
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(payload.repository) ||
    !/^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u.test(payload.tag) ||
    !Number.isSafeInteger(payload.sequence) ||
    payload.sequence <= 0 ||
    !RUN.test(payload.workflowRunId) ||
    !SOURCE.test(payload.sourceCommit) ||
    !SOURCE.test(payload.sourceTree) ||
    !HEX.test(payload.promotionEvidenceSha256) ||
    !HEX.test(payload.releaseManifestSha256) ||
    !Array.isArray(payload.objects) ||
    !Array.isArray(payload.assets) ||
    payload.objects.length === 0 ||
    payload.assets.length === 0
  )
    throw new Error('Publication manifest identity is invalid');
  for (const object of payload.objects) {
    exactObject(object, ['sha256', 'bytes', 'objectName'], 'publication object');
    if (
      !HEX.test(object.sha256) ||
      !Number.isSafeInteger(object.bytes) ||
      object.bytes < 0 ||
      object.objectName !== `sha256-${object.sha256}`
    )
      throw new Error('Publication content object is invalid');
  }
  const objectHashes = payload.objects.map(({ sha256: digest }) => digest);
  if (
    new Set(objectHashes).size !== objectHashes.length ||
    objectHashes.some((digest, index) => digest !== [...objectHashes].sort()[index])
  )
    throw new Error('Publication objects must be unique and sorted');
  const known = new Set(objectHashes);
  for (const asset of payload.assets) {
    exactObject(asset, ['name', 'objectSha256'], 'publication asset');
    if (!safeName(asset.name) || !known.has(asset.objectSha256))
      throw new Error('Publication logical asset is invalid');
  }
  const names = payload.assets.map(({ name }) => name);
  if (
    new Set(names.map((name) => name.toLowerCase())).size !== names.length ||
    names.some((name, index) => name !== [...names].sort()[index])
  )
    throw new Error('Publication logical assets must be unique and sorted');
}

export async function createPublicationManifest({
  directory,
  output,
  repository,
  tag,
  sequence,
  workflowRunId,
  sourceCommit,
  sourceTree,
  privateKeyPkcs8Base64,
  publicKeyPath,
}) {
  const outputName = basename(output);
  const { objects, assets } = await inventory(directory, new Set([outputName, 'SHA256SUMS.txt']));
  const byName = new Map(assets.map((asset) => [asset.name, asset.objectSha256]));
  const payload = {
    schemaVersion: 1,
    repository,
    tag,
    sequence: Number(sequence),
    workflowRunId,
    sourceCommit,
    sourceTree,
    objects,
    assets,
    // Retain the signed envelope format used by installed clients. Fresh releases
    // bind an explicitly automated-only report, not invented manual evidence.
    promotionEvidenceSha256:
      byName.get('windows-promotion-lifecycle-evidence-v1.json') ??
      byName.get('windows-release-validation.json'),
    releaseManifestSha256: byName.get('release-manifest.json'),
  };
  validatePayload(payload);
  const pinned = Buffer.from((await readFile(publicKeyPath, 'utf8')).trim(), 'hex');
  if (pinned.length !== 65 || pinned[0] !== 4) throw new Error('Pinned publication key is invalid');
  const privateKey = createPrivateKey({
    key: Buffer.from(privateKeyPkcs8Base64, 'base64'),
    format: 'der',
    type: 'pkcs8',
  });
  if (!publicSec1(privateKey).equals(pinned))
    throw new Error('Publication signing secret does not match its dedicated repository pin');
  const keyId = sha256(pinned);
  const signature = signBytes(
    'sha256',
    Buffer.concat([DOMAIN, Buffer.from(canonicalJson(payload))]),
    { key: privateKey, dsaEncoding: 'ieee-p1363' },
  ).toString('base64');
  const envelope = {
    payload,
    signature: { scheme: 'p256-sha256-p1363-v1', keyId, value: signature },
  };
  await writeFile(output, `${canonicalJson(envelope)}\n`, { encoding: 'utf8', mode: 0o600 });
  return envelope;
}

export async function verifyPublicationEnvelope({ envelope, repository, tag, publicKeyPath }) {
  exactObject(envelope, ['payload', 'signature'], 'publication manifest');
  exactObject(envelope.signature, ['scheme', 'keyId', 'value'], 'publication signature');
  validatePayload(envelope.payload);
  if (envelope.payload.repository !== repository || envelope.payload.tag !== tag)
    throw new Error('Publication manifest release identity does not match its container');
  const pinned = Buffer.from((await readFile(publicKeyPath, 'utf8')).trim(), 'hex');
  const keyId = sha256(pinned);
  if (
    pinned.length !== 65 ||
    pinned[0] !== 4 ||
    envelope.signature.scheme !== 'p256-sha256-p1363-v1' ||
    envelope.signature.keyId !== keyId
  )
    throw new Error('Publication signature identity is invalid');
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    pinned,
  ]);
  const key = createPublicKey({ key: spki, format: 'der', type: 'spki' });
  const signature = Buffer.from(envelope.signature.value, 'base64');
  if (
    signature.length !== 64 ||
    !verifyBytes(
      'sha256',
      Buffer.concat([DOMAIN, Buffer.from(canonicalJson(envelope.payload))]),
      { key, dsaEncoding: 'ieee-p1363' },
      signature,
    )
  )
    throw new Error('Publication manifest signature is invalid');
  return envelope;
}

export async function verifyPublicationManifest({
  path,
  directory,
  repository,
  tag,
  sequence,
  workflowRunId,
  sourceCommit,
  sourceTree,
  publicKeyPath,
}) {
  const envelope = await verifyPublicationEnvelope({
    envelope: JSON.parse(await readFile(path, 'utf8')),
    repository,
    tag,
    publicKeyPath,
  });
  if (
    envelope.payload.sequence !== Number(sequence) ||
    envelope.payload.workflowRunId !== workflowRunId ||
    envelope.payload.sourceCommit !== sourceCommit ||
    envelope.payload.sourceTree !== sourceTree
  )
    throw new Error('Publication manifest context does not match the protected release request');
  const actual = await inventory(directory, new Set([basename(path), 'SHA256SUMS.txt']));
  if (
    canonicalJson(actual.objects) !== canonicalJson(envelope.payload.objects) ||
    canonicalJson(actual.assets) !== canonicalJson(envelope.payload.assets)
  )
    throw new Error('Publication content-addressed inventory changed');
  return envelope;
}

function option(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const common = {
    directory: resolve(option('--directory')),
    repository: option('--repository'),
    tag: option('--tag'),
    sequence: option('--sequence'),
    workflowRunId: option('--run-id'),
    sourceCommit: option('--source-commit'),
    sourceTree: option('--source-tree'),
    publicKeyPath: resolve(option('--public-key')),
  };
  if (process.argv.includes('--create')) {
    await createPublicationManifest({
      ...common,
      output: resolve(option('--output')),
      privateKeyPkcs8Base64:
        process.env.TALKING_QUILL_RELEASE_MANIFEST_SIGNING_KEY_PKCS8_BASE64 ?? '',
    });
  } else {
    await verifyPublicationManifest({ ...common, path: resolve(option('--manifest')) });
  }
}
