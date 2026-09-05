import { createHash, createPublicKey, verify } from 'node:crypto';

const DOMAIN = Buffer.from('TalkingQuill/release-publication-manifest/v1\0');
const PUBLIC_KEY_SEC1 =
  '04c0fbbca85c84c8e18ec66604e70dec20cc7e79ebb4b0804c5ede060e0c51638bd82e8fd99f85301bb8acfa113389d8c1629154fcd4fc91d345eba87e4bdacc54';
const HEX = /^[0-9a-f]{64}$/u;
const TAG = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;

import type { PublicationEnvelope, PublicationPayload } from './publication-selection';

export function verifyPublicationEnvelope(
  value: unknown,
  repository: string,
  tag: string,
): PublicationEnvelope {
  const envelope = exactObject(value, ['payload', 'signature'], 'publication manifest');
  const payload = validatePayload(envelope.payload);
  const signature = exactObject(envelope.signature, ['scheme', 'keyId', 'value'], 'signature');
  const pinned = Buffer.from(PUBLIC_KEY_SEC1, 'hex');
  const keyId = createHash('sha256').update(pinned).digest('hex');
  if (
    payload.repository !== repository ||
    payload.tag !== tag ||
    signature.scheme !== 'p256-sha256-p1363-v1' ||
    signature.keyId !== keyId ||
    typeof signature.value !== 'string'
  ) {
    throw new Error('Publication signature identity is invalid');
  }
  const bytes = Buffer.from(signature.value, 'base64');
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    pinned,
  ]);
  if (
    bytes.length !== 64 ||
    !verify(
      'sha256',
      Buffer.concat([DOMAIN, Buffer.from(canonicalJson(payload))]),
      {
        key: createPublicKey({ key: spki, format: 'der', type: 'spki' }),
        dsaEncoding: 'ieee-p1363',
      },
      bytes,
    )
  ) {
    throw new Error('Publication signature is invalid');
  }
  return { payload, signature: signature as PublicationEnvelope['signature'] };
}

function validatePayload(value: unknown): PublicationPayload {
  const payload = exactObject(
    value,
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
    'payload',
  );
  if (
    payload.schemaVersion !== 1 ||
    typeof payload.repository !== 'string' ||
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(payload.repository) ||
    typeof payload.tag !== 'string' ||
    !TAG.test(payload.tag) ||
    !Number.isSafeInteger(payload.sequence) ||
    (payload.sequence as number) <= 0 ||
    typeof payload.workflowRunId !== 'string' ||
    !/^[1-9]\d*$/u.test(payload.workflowRunId) ||
    typeof payload.sourceCommit !== 'string' ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceCommit) ||
    typeof payload.sourceTree !== 'string' ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceTree) ||
    typeof payload.promotionEvidenceSha256 !== 'string' ||
    !HEX.test(payload.promotionEvidenceSha256) ||
    typeof payload.releaseManifestSha256 !== 'string' ||
    !HEX.test(payload.releaseManifestSha256) ||
    !Array.isArray(payload.objects) ||
    !Array.isArray(payload.assets)
  )
    throw new Error('Publication payload identity is invalid');
  const objects = payload.objects.map((item) => {
    const object = exactObject(item, ['sha256', 'bytes', 'objectName'], 'object');
    if (
      typeof object.sha256 !== 'string' ||
      !HEX.test(object.sha256) ||
      !Number.isSafeInteger(object.bytes) ||
      (object.bytes as number) < 0 ||
      object.objectName !== `sha256-${object.sha256}`
    )
      throw new Error('Publication object is invalid');
    return object as unknown as PublicationPayload['objects'][number];
  });
  const hashes = objects.map(({ sha256 }) => sha256);
  if (
    new Set(hashes).size !== hashes.length ||
    hashes.some((hash, index) => hash !== [...hashes].sort()[index])
  )
    throw new Error('Publication objects are not unique and sorted');
  const known = new Set(hashes);
  const assets = payload.assets.map((item) => {
    const asset = exactObject(item, ['name', 'objectSha256'], 'asset');
    if (
      typeof asset.name !== 'string' ||
      !safeName(asset.name) ||
      typeof asset.objectSha256 !== 'string' ||
      !known.has(asset.objectSha256)
    )
      throw new Error('Publication asset is invalid');
    return asset as unknown as PublicationPayload['assets'][number];
  });
  const names = assets.map(({ name }) => name);
  if (
    new Set(names.map((name) => name.toLowerCase())).size !== names.length ||
    names.some((name, index) => name !== [...names].sort()[index])
  )
    throw new Error('Publication assets are not unique and sorted');
  return { ...(payload as unknown as PublicationPayload), objects, assets };
}

function exactObject(
  value: unknown,
  keys: readonly string[],
  label: string,
): Record<string, unknown> {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error(`${label} must be an object`);
  const object = value as Record<string, unknown>;
  const actual = Object.keys(object).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index]))
    throw new Error(`${label} schema is invalid`);
  return object;
}

function safeName(value: string): boolean {
  return (
    value.length > 0 &&
    value.length <= 255 &&
    !/[\\/\p{C}]/u.test(value) &&
    value !== '.' &&
    value !== '..'
  );
}

function canonicalJson(value: unknown): string {
  if (value === null || typeof value === 'boolean' || typeof value === 'string')
    return JSON.stringify(value);
  if (typeof value === 'number' && Number.isFinite(value)) return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (typeof value === 'object')
    return `{${Object.keys(value as Record<string, unknown>)
      .sort()
      .map(
        (key) => `${JSON.stringify(key)}:${canonicalJson((value as Record<string, unknown>)[key])}`,
      )
      .join(',')}}`;
  throw new Error('Publication manifest contains a non-JSON value');
}
