import { createHash } from 'node:crypto';
import { z } from 'zod';
import { PinnedJsonTransport, type JsonTransport } from '../providers/json-transport';
import { RELEASE_REPOSITORY, validateReleaseUrl } from './release-url-policy';
import {
  selectCompatiblePublication,
  type ImmutablePublicationRelease,
  type InstalledWindowsUpdateIdentity,
  type PublicationAsset,
  type SelectedPublication,
} from './publication-selection';
import { parseUnsignedUpdateIdentity } from './unsigned-update-identity';
import { parseVerifiedChannel } from './signed-publication-provider';

const AssetSchema = z.looseObject({
  name: z.string().min(1).max(255),
  size: z
    .number()
    .int()
    .nonnegative()
    .max(2 ** 40),
  browser_download_url: z.url().max(2_048),
});
const ReleaseSchema = z.looseObject({
  id: z.number().int().positive(),
  tag_name: z.string().min(1).max(64),
  html_url: z.url().max(2_048),
  draft: z.boolean(),
  prerelease: z.boolean(),
  immutable: z.boolean(),
  assets: z.array(AssetSchema).max(128),
});

export interface VerifiedPublication extends SelectedPublication {
  readonly channelBytes: Buffer;
}

export class PublicationCatalog {
  readonly #transport: JsonTransport;

  constructor(
    transport: JsonTransport = new PinnedJsonTransport(undefined, { category: 'update' }),
  ) {
    this.#transport = transport;
  }

  async select(
    architecture: 'x64' | 'arm64',
    installed: InstalledWindowsUpdateIdentity,
  ): Promise<VerifiedPublication> {
    const controller = new AbortController();
    const releases: ImmutablePublicationRelease[] = [];
    for (let page = 1; page <= 10; page += 1) {
      const response = await this.#transport.request({
        url: `https://api.github.com/repos/${RELEASE_REPOSITORY}/releases?per_page=100&page=${String(page)}`,
        method: 'GET',
        headers: githubHeaders(),
        credentialed: false,
        fixedCloud: true,
        allowedOrigins: ['https://api.github.com'],
        signal: controller.signal,
        timeoutMs: 30_000,
        maxResponseBytes: 4 * 1024 * 1024,
        maxOperationResponseBytes: 16 * 1024 * 1024,
      });
      const values = z.array(ReleaseSchema).max(100).parse(response.body);
      releases.push(...values);
      if (values.length < 100) break;
      if (page === 10) throw new Error('Immutable publication history exceeds its bound');
    }
    let compatibilityCandidates = 0;
    let compatibilityBytes = 0;
    const selected = await selectCompatiblePublication(
      releases,
      RELEASE_REPOSITORY,
      architecture,
      installed,
      async (asset, tag) => await this.#loadManifest(asset, tag),
      async (publication) => {
        const channelBytes = await this.#loadBytes(
          publication.channelAsset,
          publication.release.tag_name,
          4 * 1024 * 1024,
        );
        compatibilityCandidates += 1;
        compatibilityBytes += channelBytes.length;
        if (compatibilityCandidates > 64 || compatibilityBytes > 16 * 1024 * 1024)
          throw new Error('Compatible publication search exceeds its bound');
        const verified = bindVerifiedChannel(publication, channelBytes);
        const info = parseVerifiedChannel(verified);
        const identity = parseUnsignedUpdateIdentity(
          (info as unknown as { talkingQuillRelease?: unknown }).talkingQuillRelease,
          'win32',
          architecture,
          info.version,
        );
        if (identity.predecessor === null)
          throw new Error('Signed Windows update publication has no predecessor');
        return {
          predecessor: {
            version: identity.predecessor.version,
            architecture,
            releaseBuildDigest: identity.predecessor.releaseBuildDigest,
            gatewaySha256: identity.predecessor.gatewaySha256,
            ownerSha256: identity.predecessor.ownerSha256,
          },
          value: verified,
        };
      },
    );
    validateReleaseUrl(
      selected.publication.release.html_url,
      selected.publication.release.tag_name,
    );
    validateAssetUrl(
      selected.publication.packageAsset.browser_download_url,
      selected.publication.packageAsset.name,
      selected.publication.release.tag_name,
    );
    return selected.value;
  }

  async #loadManifest(asset: PublicationAsset, tag: string): Promise<unknown> {
    if (asset.size <= 0 || asset.size > 1024 * 1024)
      throw new Error('Publication manifest size is invalid');
    const bytes = await this.#loadBytes(asset, tag, 1024 * 1024);
    if (bytes.length !== asset.size) throw new Error('Publication manifest size changed');
    try {
      return JSON.parse(bytes.toString('utf8')) as unknown;
    } catch {
      throw new Error('Publication manifest JSON is invalid');
    }
  }

  async #loadBytes(
    asset: PublicationAsset,
    expectedTag: string | undefined,
    maxResponseBytes: number,
  ): Promise<Buffer> {
    const signal = new AbortController().signal;
    validateAssetUrl(asset.browser_download_url, asset.name, expectedTag);
    const response = await this.#transport.request({
      url: asset.browser_download_url,
      method: 'GET',
      headers: githubHeaders(),
      credentialed: false,
      fixedCloud: true,
      allowedOrigins: [
        'https://github.com',
        'https://release-assets.githubusercontent.com',
        'https://objects.githubusercontent.com',
      ],
      signal,
      timeoutMs: 30_000,
      maxResponseBytes,
      responseType: 'bytes',
      maxOperationResponseBytes: 16 * 1024 * 1024,
    });
    if (!Buffer.isBuffer(response.body)) throw new Error('Publication asset bytes are unavailable');
    return response.body;
  }
}

export function bindVerifiedChannel(
  selected: SelectedPublication,
  channelBytes: Buffer,
): VerifiedPublication {
  if (
    channelBytes.length !== selected.channelAsset.size ||
    createHash('sha256').update(channelBytes).digest('hex') !== selected.channelSha256
  )
    throw new Error('Channel metadata does not match its signed publication object');
  return { ...selected, channelBytes: Buffer.from(channelBytes) };
}

export function validateAssetUrl(value: string, name: string, expectedTag?: string): void {
  const url = new URL(value);
  const expectedPrefix = `/${RELEASE_REPOSITORY}/releases/download/`;
  if (
    value !== url.toString() ||
    url.protocol !== 'https:' ||
    url.hostname !== 'github.com' ||
    url.port !== '' ||
    url.username !== '' ||
    url.password !== '' ||
    url.search !== '' ||
    url.hash !== '' ||
    !url.pathname.startsWith(expectedPrefix) ||
    (expectedTag !== undefined && !url.pathname.startsWith(`${expectedPrefix}${expectedTag}/`)) ||
    decodeURIComponent(url.pathname.split('/').at(-1) ?? '') !== name
  )
    throw new Error('Publication asset URL is invalid');
}

function githubHeaders(): Readonly<Record<string, string>> {
  return {
    accept: 'application/vnd.github+json',
    'user-agent': 'Talking-Quill/updater',
    'x-github-api-version': '2022-11-28',
  };
}
