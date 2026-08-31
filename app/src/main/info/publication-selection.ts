import { createHash, createPublicKey, verify } from 'node:crypto';

const DOMAIN = Buffer.from('TalkingQuill/release-publication-manifest/v1\0');
const PUBLIC_KEY_SEC1 =
  '04c0fbbca85c84c8e18ec66604e70dec20cc7e79ebb4b0804c5ede060e0c51638bd82e8fd99f85301bb8acfa113389d8c1629154fcd4fc91d345eba87e4bdacc54';
const HEX = /^[0-9a-f]{64}$/u;
const TAG = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;

export interface PublicationAsset {
  readonly name: string;
  readonly size: number;
  readonly browser_download_url: string;
}

export interface ImmutablePublicationRelease {
  readonly id: number;
  readonly tag_name: string;
  readonly draft: boolean;
  readonly prerelease: boolean;
  readonly immutable: boolean;
  readonly assets: readonly PublicationAsset[];
}

export interface PublicationPayload {
  readonly schemaVersion: number;
  readonly repository: string;
  readonly tag: string;
  readonly sequence: number;
  readonly workflowRunId: string;
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly objects: readonly {
    readonly sha256: string;
    readonly bytes: number;
    readonly objectName: string;
  }[];
  readonly assets: readonly { readonly name: string; readonly objectSha256: string }[];
  readonly promotionEvidenceSha256: string;
  readonly releaseManifestSha256: string;
}

export interface PublicationEnvelope {
  readonly payload: PublicationPayload;
  readonly signature: { readonly scheme: string; readonly keyId: string; readonly value: string };
}

export interface SelectedPublication {
  readonly release: ImmutablePublicationRelease;
  readonly payload: PublicationPayload;
  readonly version: string;
  readonly channelAsset: PublicationAsset;
  readonly packageAsset: PublicationAsset;
  readonly packageSha256: string;
}

export async function selectHighestPublication(
  releases: readonly ImmutablePublicationRelease[],
  repository: string,
  architecture: 'x64' | 'arm64',
  loadManifest: (asset: PublicationAsset) => Promise<unknown>,
  verifyEnvelope: (
    value: unknown,
    repository: string,
    tag: string,
  ) => PublicationEnvelope = verifyPublicationEnvelope,
): Promise<SelectedPublication> {
  const immutable = releases.filter(
    (release) => !release.draft && !release.prerelease && release.immutable,
  );
  if (immutable.length === 0) throw new Error('No immutable signed publication is available');
  const verified: {
    release: ImmutablePublicationRelease;
    payload: PublicationPayload;
    version: string;
  }[] = [];
  for (const release of immutable) {
    const manifests = release.assets.filter(
      ({ name }) => name === 'release-publication-manifest-v1.json',
    );
    if (manifests.length !== 1)
      throw new Error('Immutable release manifest is missing or ambiguous');
    const manifest = manifests.at(0);
    if (manifest === undefined)
      throw new Error('Immutable release manifest is missing or ambiguous');
    const envelope = verifyEnvelope(await loadManifest(manifest), repository, release.tag_name);
    verifyReleaseInventory(release, envelope.payload);
    verified.push({ release, payload: envelope.payload, version: release.tag_name.slice(1) });
  }
  verified.sort((left, right) => left.payload.sequence - right.payload.sequence);
  for (let index = 1; index < verified.length; index += 1) {
    const previous = verified[index - 1];
    const current = verified[index];
    if (
      previous === undefined ||
      current === undefined ||
      current.payload.sequence <= previous.payload.sequence ||
      compareSemver(current.version, previous.version) <= 0
    ) {
      throw new Error('Immutable publication history is not strictly monotonic');
    }
  }
  const selected = verified.at(-1);
  if (selected === undefined) throw new Error('No immutable signed publication is available');
  const channelName = `latest-${architecture}.yml`;
  const channelAsset = uniqueAsset(selected.release, channelName);
  const packageName = `Talking-Quill-${selected.version}-win-${architecture}-update.exe`;
  const logicalPackage = selected.payload.assets.filter(({ name }) => name === packageName);
  if (logicalPackage.length !== 1)
    throw new Error('Windows publication package is missing or ambiguous');
  const packageAsset = uniqueAsset(selected.release, packageName);
  return {
    ...selected,
    channelAsset,
    packageAsset,
    packageSha256: logicalPackage[0]?.objectSha256 ?? '',
  };
}

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

function verifyReleaseInventory(
  release: ImmutablePublicationRelease,
  payload: PublicationPayload,
): void {
  for (const logical of payload.assets) {
    const object = payload.objects.find(({ sha256 }) => sha256 === logical.objectSha256);
    const asset = uniqueAsset(release, logical.name);
    if (asset.size !== object?.bytes)
      throw new Error('Published asset inventory does not match its signed object');
  }
}

function uniqueAsset(release: ImmutablePublicationRelease, name: string): PublicationAsset {
  const matches = release.assets.filter((asset) => asset.name === name);
  if (matches.length !== 1) throw new Error(`Published asset is missing or ambiguous: ${name}`);
  const match = matches.at(0);
  if (match === undefined) throw new Error(`Published asset is missing: ${name}`);
  return match;
}

function compareSemver(left: string, right: string): number {
  const a = TAG.exec(`v${left}`);
  const b = TAG.exec(`v${right}`);
  if (a === null || b === null) throw new Error('Publication version is invalid');
  for (let index = 1; index <= 3; index += 1) {
    const x = BigInt(a[index] ?? '0');
    const y = BigInt(b[index] ?? '0');
    if (x !== y) return x > y ? 1 : -1;
  }
  return 0;
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
