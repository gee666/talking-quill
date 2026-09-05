import { verifyPublicationEnvelope } from './publication-envelope';
export { verifyPublicationEnvelope } from './publication-envelope';

const TAG = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u;

export interface PublicationAsset {
  readonly name: string;
  readonly size: number;
  readonly browser_download_url: string;
}

export interface ImmutablePublicationRelease {
  readonly id: number;
  readonly tag_name: string;
  readonly html_url: string;
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

export interface InstalledWindowsUpdateIdentity {
  readonly version: string;
  readonly architecture: 'x64' | 'arm64';
  readonly releaseBuildDigest: string;
  readonly gatewaySha256: string;
  readonly ownerSha256: string;
}

export interface SelectedPublication {
  readonly release: ImmutablePublicationRelease;
  readonly payload: PublicationPayload;
  readonly version: string;
  readonly channelAsset: PublicationAsset;
  readonly channelSha256: string;
  readonly packageAsset: PublicationAsset;
  readonly packageSha256: string;
}

export async function selectHighestPublication(
  releases: readonly ImmutablePublicationRelease[],
  repository: string,
  architecture: 'x64' | 'arm64',
  loadManifest: (asset: PublicationAsset, tag: string) => Promise<unknown>,
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
    const envelope = verifyEnvelope(
      await loadManifest(manifest, release.tag_name),
      repository,
      release.tag_name,
    );
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
    channelSha256:
      selected.payload.assets.find(({ name }) => name === channelName)?.objectSha256 ?? '',
    packageAsset,
    packageSha256: logicalPackage[0]?.objectSha256 ?? '',
  };
}

export interface CompatiblePublication<T> {
  readonly publication: SelectedPublication;
  readonly value: T;
}

export async function selectCompatiblePublication<T>(
  releases: readonly ImmutablePublicationRelease[],
  repository: string,
  architecture: 'x64' | 'arm64',
  installed: InstalledWindowsUpdateIdentity,
  loadManifest: (asset: PublicationAsset, tag: string) => Promise<unknown>,
  loadCompatibility: (
    publication: SelectedPublication,
  ) => Promise<{ readonly predecessor: InstalledWindowsUpdateIdentity; readonly value: T }>,
  verifyEnvelope: (
    value: unknown,
    repository: string,
    tag: string,
  ) => PublicationEnvelope = verifyPublicationEnvelope,
): Promise<CompatiblePublication<T>> {
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
    const envelope = verifyEnvelope(
      await loadManifest(manifest, release.tag_name),
      repository,
      release.tag_name,
    );
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
    )
      throw new Error('Immutable publication history is not strictly monotonic');
  }
  for (const candidate of verified) {
    if (compareSemver(candidate.version, installed.version) <= 0) continue;
    const publication = materializePublication(candidate, architecture);
    const compatibility = await loadCompatibility(publication);
    if (sameInstalledIdentity(compatibility.predecessor, installed)) {
      return { publication, value: compatibility.value };
    }
  }
  throw new Error('No compatible signed Windows update publication is available');
}

function materializePublication(
  selected: {
    readonly release: ImmutablePublicationRelease;
    readonly payload: PublicationPayload;
    readonly version: string;
  },
  architecture: 'x64' | 'arm64',
): SelectedPublication {
  const channelName = `latest-${architecture}.yml`;
  const channelAsset = uniqueAsset(selected.release, channelName);
  const packageName = `Talking-Quill-${selected.version}-win-${architecture}-update.exe`;
  const logicalPackage = selected.payload.assets.filter(({ name }) => name === packageName);
  if (logicalPackage.length !== 1)
    throw new Error('Windows publication package is missing or ambiguous');
  return {
    ...selected,
    channelAsset,
    channelSha256:
      selected.payload.assets.find(({ name }) => name === channelName)?.objectSha256 ?? '',
    packageAsset: uniqueAsset(selected.release, packageName),
    packageSha256: logicalPackage[0]?.objectSha256 ?? '',
  };
}

function sameInstalledIdentity(
  left: InstalledWindowsUpdateIdentity,
  right: InstalledWindowsUpdateIdentity,
): boolean {
  return (
    left.version === right.version &&
    left.architecture === right.architecture &&
    left.releaseBuildDigest === right.releaseBuildDigest &&
    left.gatewaySha256 === right.gatewaySha256 &&
    left.ownerSha256 === right.ownerSha256
  );
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
