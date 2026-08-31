import { z } from 'zod';
import { PinnedJsonTransport, type JsonTransport } from '../providers/json-transport';
import { RELEASE_REPOSITORY } from './release-url-policy';
import {
  selectHighestPublication,
  type ImmutablePublicationRelease,
  type PublicationAsset,
  type SelectedPublication,
} from './publication-selection';

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
  draft: z.boolean(),
  prerelease: z.boolean(),
  immutable: z.boolean(),
  assets: z.array(AssetSchema).max(128),
});

export class PublicationCatalog {
  readonly #transport: JsonTransport;

  constructor(
    transport: JsonTransport = new PinnedJsonTransport(undefined, { category: 'update' }),
  ) {
    this.#transport = transport;
  }

  async select(architecture: 'x64' | 'arm64'): Promise<SelectedPublication> {
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
    return await selectHighestPublication(
      releases,
      RELEASE_REPOSITORY,
      architecture,
      async (asset) => await this.#loadManifest(asset),
    );
  }

  async #loadManifest(asset: PublicationAsset): Promise<unknown> {
    const signal = new AbortController().signal;
    validateAssetUrl(asset.browser_download_url, asset.name);
    if (asset.size <= 0 || asset.size > 1024 * 1024)
      throw new Error('Publication manifest size is invalid');
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
      maxResponseBytes: 1024 * 1024,
      allowOctetStreamJson: true,
      maxOperationResponseBytes: 16 * 1024 * 1024,
    });
    return response.body;
  }
}

function validateAssetUrl(value: string, name: string): void {
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
