import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import {
  selectHighestPublication,
  type ImmutablePublicationRelease,
  type PublicationEnvelope,
  type PublicationPayload,
} from '../../app/src/main/info/publication-selection';

const digest = (digit: string): string => digit.repeat(64);

function publication(
  sequence: number,
  version: string,
  digit: string,
): {
  release: ImmutablePublicationRelease;
  envelope: PublicationEnvelope;
} {
  const channel = `latest-x64.yml`;
  const packageName = `Talking-Quill-${version}-win-x64-update.exe`;
  const otherPackageName = `Talking-Quill-${version}-win-arm64-update.exe`;
  const objects = [
    { sha256: digest(digit), bytes: 10, objectName: `sha256-${digest(digit)}` },
    {
      sha256: digest(String((Number(digit) + 1) % 10)),
      bytes: 20,
      objectName: `sha256-${digest(String((Number(digit) + 1) % 10))}`,
    },
  ].sort((left, right) => left.sha256.localeCompare(right.sha256));
  const packageHash = objects.find(({ bytes }) => bytes === 20)?.sha256 ?? '';
  const channelHash = objects.find(({ bytes }) => bytes === 10)?.sha256 ?? '';
  const payload: PublicationPayload = {
    schemaVersion: 1,
    repository: 'gee666/talking-quill',
    tag: `v${version}`,
    sequence,
    workflowRunId: String(sequence),
    sourceCommit: 'a'.repeat(40),
    sourceTree: 'b'.repeat(40),
    objects,
    assets: [
      { name: packageName, objectSha256: packageHash },
      { name: otherPackageName, objectSha256: packageHash },
      { name: channel, objectSha256: channelHash },
    ].sort((left, right) => left.name.localeCompare(right.name)),
    promotionEvidenceSha256: digest('c'),
    releaseManifestSha256: digest('d'),
  };
  return {
    envelope: {
      payload,
      signature: { scheme: 'test', keyId: 'test', value: 'test' },
    },
    release: {
      id: sequence,
      tag_name: `v${version}`,
      draft: false,
      prerelease: false,
      immutable: true,
      assets: [
        { name: channel, size: 10, browser_download_url: `https://example/${channel}` },
        { name: packageName, size: 20, browser_download_url: `https://example/${packageName}` },
        {
          name: otherPackageName,
          size: 20,
          browser_download_url: `https://example/${otherPackageName}`,
        },
        {
          name: 'release-publication-manifest-v1.json',
          size: 100,
          browser_download_url: `https://example/manifest-${String(sequence)}`,
        },
      ],
    },
  };
}

const verifier = (value: unknown): PublicationEnvelope => value as PublicationEnvelope;

describe('runtime immutable publication selection', () => {
  it('pins the dedicated repository publication key in the updater verifier', async () => {
    const [source, pin] = await Promise.all([
      readFile('app/src/main/info/publication-selection.ts', 'utf8'),
      readFile('build/release-manifest-public-key.sec1', 'utf8'),
    ]);
    expect(source).toContain(pin.trim());
  });

  it('selects the highest signed monotonic sequence even when an older release publishes later', async () => {
    const old = publication(41, '0.0.69', '1');
    const high = publication(43, '0.0.71', '5');
    const concurrentOlder = publication(42, '0.0.70', '3');
    const envelopes = new Map([
      [old.release.id, old.envelope],
      [high.release.id, high.envelope],
      [concurrentOlder.release.id, concurrentOlder.envelope],
    ]);
    const selected = await selectHighestPublication(
      [old.release, high.release, concurrentOlder.release],
      'gee666/talking-quill',
      'x64',
      (asset) => {
        const sequence = Number(asset.browser_download_url.split('-').at(-1));
        return Promise.resolve(envelopes.get(sequence));
      },
      verifier,
    );
    expect(selected.release.id).toBe(43);
    expect(selected.version).toBe('0.0.71');
    expect(selected.packageSha256).toBe(
      high.envelope.payload.assets.find(({ name }) => name.includes('win-x64-update.exe'))
        ?.objectSha256,
    );
  });

  it('rejects a higher sequence carrying an equal or lower version', async () => {
    const first = publication(41, '0.0.70', '1');
    const rollback = publication(42, '0.0.69', '3');
    const values = [first, rollback];
    await expect(
      selectHighestPublication(
        values.map(({ release }) => release),
        'gee666/talking-quill',
        'x64',
        (asset) =>
          Promise.resolve(
            values.find(({ release }) => asset.browser_download_url.endsWith(String(release.id)))
              ?.envelope,
          ),
        verifier,
      ),
    ).rejects.toThrow('strictly monotonic');
  });

  it('requires a signed manifest on every immutable release', async () => {
    const value = publication(41, '0.0.69', '1');
    const unsigned = { ...value.release, assets: value.release.assets.slice(0, 2) };
    await expect(
      selectHighestPublication(
        [unsigned],
        'gee666/talking-quill',
        'x64',
        () => Promise.resolve(value.envelope),
        verifier,
      ),
    ).rejects.toThrow('missing or ambiguous');
  });
});
