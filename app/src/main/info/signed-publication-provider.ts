import { Provider, type AppUpdater } from 'electron-updater';
import {
  parseUpdateInfo,
  type ProviderRuntimeOptions,
} from 'electron-updater/out/providers/Provider';
import type { VerifiedPublication } from './publication-catalog';

interface SignedProviderOptions {
  readonly provider: 'custom';
  readonly updateProvider: typeof SignedPublicationProvider;
  readonly publication: VerifiedPublication;
}

type ChannelUpdateInfo = ReturnType<typeof parseUpdateInfo>;

export class SignedPublicationProvider extends Provider<ChannelUpdateInfo> {
  readonly #publication: VerifiedPublication;
  readonly #updateInfo: ChannelUpdateInfo;

  constructor(
    options: SignedProviderOptions,
    _updater: AppUpdater,
    runtimeOptions: ProviderRuntimeOptions,
  ) {
    super(runtimeOptions);
    this.#publication = options.publication;
    this.#updateInfo = parseVerifiedChannel(options.publication);
  }

  getLatestVersion(): Promise<ChannelUpdateInfo> {
    return Promise.resolve(this.#updateInfo);
  }

  resolveFiles(updateInfo: ChannelUpdateInfo) {
    if (updateInfo !== this.#updateInfo) throw new Error('Updater metadata instance changed');
    const files = updateInfo.files;
    const selected = files.filter(({ url }) => url === this.#publication.packageAsset.name);
    if (selected.length !== 1) throw new Error('Verified updater package metadata is ambiguous');
    const info = selected.at(0);
    if (info === undefined) throw new Error('Verified updater package metadata is missing');
    return [
      {
        url: new URL(this.#publication.packageAsset.browser_download_url),
        info,
      },
    ];
  }

  override getBlockMapFiles(): URL[] {
    return [];
  }
}

export function signedPublicationProviderOptions(
  publication: VerifiedPublication,
): SignedProviderOptions {
  return {
    provider: 'custom',
    updateProvider: SignedPublicationProvider,
    publication,
  };
}

export function parseVerifiedChannel(publication: VerifiedPublication): ChannelUpdateInfo {
  const decoded = new TextDecoder('utf-8', { fatal: true }).decode(publication.channelBytes);
  const info = parseUpdateInfo(
    decoded,
    publication.channelAsset.name,
    new URL(publication.channelAsset.browser_download_url),
  );
  if (info.version !== publication.version || !Array.isArray(info.files))
    throw new Error('Verified updater channel identity does not match its publication');
  const selected = info.files.filter((file) => file.url === publication.packageAsset.name);
  if (
    selected.length !== 1 ||
    info.files.some(
      (file) =>
        typeof file.url !== 'string' ||
        file.url.includes('/') ||
        file.url.includes('\\') ||
        file.url.includes('?') ||
        file.url.includes('#'),
    )
  )
    throw new Error('Verified updater channel asset list is invalid');
  return info;
}
