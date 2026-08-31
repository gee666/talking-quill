import { createHash } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import {
  validateAssetUrl,
  type VerifiedPublication,
} from '../../app/src/main/info/publication-catalog';
import {
  SignedPublicationProvider,
  signedPublicationProviderOptions,
} from '../../app/src/main/info/signed-publication-provider';

function fixture(metadata: string): VerifiedPublication {
  const packageName = 'Talking-Quill-0.0.69-win-x64-update.exe';
  const channelName = 'latest-x64.yml';
  const channelBytes = Buffer.from(metadata);
  return {
    release: {
      id: 69,
      tag_name: 'v0.0.69',
      draft: false,
      prerelease: false,
      immutable: true,
      assets: [],
    },
    payload: {
      schemaVersion: 1,
      repository: 'gee666/talking-quill',
      tag: 'v0.0.69',
      sequence: 69,
      workflowRunId: '69',
      sourceCommit: 'a'.repeat(40),
      sourceTree: 'b'.repeat(40),
      objects: [],
      assets: [],
      promotionEvidenceSha256: 'c'.repeat(64),
      releaseManifestSha256: 'd'.repeat(64),
    },
    version: '0.0.69',
    channelAsset: {
      name: channelName,
      size: channelBytes.length,
      browser_download_url: `https://github.com/gee666/talking-quill/releases/download/v0.0.69/${channelName}`,
    },
    channelSha256: createHash('sha256').update(channelBytes).digest('hex'),
    channelBytes,
    packageAsset: {
      name: packageName,
      size: 123,
      browser_download_url: `https://github.com/gee666/talking-quill/releases/download/v0.0.69/${packageName}`,
    },
    packageSha256: 'e'.repeat(64),
  };
}

function provider(publication: VerifiedPublication): SignedPublicationProvider {
  const executor = { request: vi.fn(() => Promise.reject(new Error('network forbidden'))) };
  return new SignedPublicationProvider(
    signedPublicationProviderOptions(publication),
    {} as never,
    { executor, isUseMultipleRangeRequest: false, platform: 'win32' } as never,
  );
}

const validMetadata = `version: 0.0.69
files:
  - url: Talking-Quill-0.0.69-win-x64-update.exe
    sha512: ${'A'.repeat(88)}
path: Talking-Quill-0.0.69-win-x64-update.exe
sha512: ${'A'.repeat(88)}
releaseDate: '2026-08-31T00:00:00.000Z'
`;

describe('signed publication electron-updater provider', () => {
  it('parses retained verified bytes without a metadata network request', async () => {
    const value = provider(fixture(validMetadata));
    const info = await value.getLatestVersion();
    expect(info.version).toBe('0.0.69');
    expect(value.resolveFiles(info)).toEqual([
      expect.objectContaining({
        url: new URL(
          'https://github.com/gee666/talking-quill/releases/download/v0.0.69/Talking-Quill-0.0.69-win-x64-update.exe',
        ),
      }),
    ]);
  });

  it('rejects package download URLs outside the signed release tag', () => {
    const name = 'Talking-Quill-0.0.69-win-x64-update.exe';
    expect(() => validateAssetUrl(`https://example.com/${name}`, name, 'v0.0.69')).toThrow(
      'URL is invalid',
    );
    expect(() =>
      validateAssetUrl(
        `https://github.com/gee666/talking-quill/releases/download/v0.0.70/${name}`,
        name,
        'v0.0.69',
      ),
    ).toThrow('URL is invalid');
  });

  it('denies traversal, alternate packages, and mismatched versions', () => {
    expect(() =>
      provider(fixture(validMetadata.replace('0.0.69\nfiles', '0.0.70\nfiles'))),
    ).toThrow('does not match');
    expect(() =>
      provider(fixture(validMetadata.replace('Talking-Quill-0.0.69', '../Talking-Quill-0.0.69'))),
    ).toThrow('asset list is invalid');
    expect(() =>
      provider(fixture(validMetadata.replace('-x64-update.exe', '-arm64-update.exe'))),
    ).toThrow('asset list is invalid');
  });
});
