import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, readdir, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';
import { assembleOrdinaryUnsignedRelease } from '../../scripts/assemble-ordinary-unsigned-release.mjs';
import { validateReleaseManifest } from '../../scripts/release-manifest.mjs';
import type * as ArtifactProvenance from '../../scripts/artifact-provenance.mjs';

// Isolate source lookup and previously completed native execution. Filesystem
// staging, provenance schemas, exact hashes, manifest sealing and all publication
// CLI boundaries below are real production implementations, not mocked sealers.
vi.mock('../../scripts/source-identity.mjs', () => ({
  currentSourceIdentity: () => ({ sourceCommit: 'a'.repeat(40), sourceTree: 'b'.repeat(40) }),
}));
vi.mock('../../scripts/release-version-policy.mjs', () => ({
  verifyCoordinatedVersions: () => Promise.resolve('0.0.73'),
}));
vi.mock('../../scripts/artifact-provenance.mjs', async (importOriginal) => ({
  ...(await importOriginal<typeof ArtifactProvenance>()),
  currentSourceTreeHash: () => Promise.resolve('c'.repeat(64)),
}));
vi.mock('../../scripts/windows-hosted-runtime-evidence.mjs', () => ({
  verifyHostedRuntimeEvidence: async ({ evidencePath }: { evidencePath: string }) =>
    JSON.parse(await readFile(evidencePath, 'utf8')) as Record<string, unknown>,
}));

const roots: string[] = [];
afterEach(async () => {
  vi.unstubAllEnvs();
  await Promise.all(roots.splice(0).map(removeTestDirectory));
});
const hash = (bytes: string | Buffer) => createHash('sha256').update(bytes).digest('hex');
const commit = 'a'.repeat(40);
const tree = 'b'.repeat(40);
const run = '34021506735';
const tag = 'v0.0.73';
const cli = (script: string, args: string[]) =>
  execFileSync(process.execPath, [resolve('scripts', script), ...args], {
    encoding: 'utf8',
    timeout: 10_000,
    stdio: ['ignore', 'pipe', 'pipe'],
  });

async function fixture() {
  vi.stubEnv('GITHUB_RUN_ID', run);
  const root = await createTestDirectory('ordinary-publication-boundary');
  roots.push(root);
  const preserved = join(root, 'preserved');
  const smoke = join(root, 'smoke');
  await mkdir(smoke);
  for (const arch of ['x64', 'arm64']) {
    const directory = join(preserved, arch, 'tmp/release-upload');
    await mkdir(directory, { recursive: true });
    const installer = `Talking-Quill-0.0.73-win-${arch}-setup.exe`;
    const bytes = `${arch} fixture installer bytes`;
    await writeFile(join(directory, installer), bytes);
    await writeFile(join(directory, 'THIRD_PARTY_NOTICES.txt'), 'matching notices');
    await writeFile(
      join(directory, 'RELEASE.json'),
      JSON.stringify({
        schemaVersion: 1,
        result: 'passed',
        packageMode: 'fresh',
        variant: 'canonical',
        architecture: arch,
        version: '0.0.73',
        sourceCommit: commit,
        sourceTree: tree,
        installer,
        bytes: Buffer.byteLength(bytes),
        sha256: hash(bytes),
        tqpkg2TreeSha256: 'd'.repeat(64),
      }),
    );
    await writeFile(
      join(directory, `provenance-win-${arch}-setup.json`),
      JSON.stringify({
        schemaVersion: 2,
        sourceCommit: commit,
        sourceTree: tree,
        sourceTreeSha256: 'c'.repeat(64),
        package: { version: '0.0.73', platform: 'win', arch, root: `release/win-${arch}-unpacked` },
        entries: [
          {
            role: 'final-artifact',
            path: `release/${installer}`,
            kind: 'file',
            size: Buffer.byteLength(bytes),
            sha256: hash(bytes),
          },
        ],
      }),
    );
    await writeFile(
      join(smoke, `windows-hosted-runtime-${arch}.json`),
      JSON.stringify({ workflowRunId: run, architecture: arch }),
    );
  }
  const output = join(root, 'output');
  const manifest = await assembleOrdinaryUnsignedRelease(preserved, smoke, output);
  return { root, preserved, smoke, output, manifest };
}

describe('ordinary two-architecture assembly through publication boundaries', () => {
  it('seals deterministic setup-only provenance and stages exactly the complete ordinary upload set', async () => {
    const { root, preserved, smoke, output, manifest } = await fixture();
    expect(manifest.architecture).toBe('x64+arm64');
    expect(manifest.workflowRunId).toBe(run);
    expect(manifest.sourceCommit).toBe(commit);
    expect(manifest.sourceTree).toBe(tree);
    expect(manifest.provenance.map(({ name }) => name)).toEqual([
      'provenance-win-arm64-setup.json',
      'provenance-win-x64-setup.json',
    ]);
    expect(manifest.assets).toHaveLength(9);
    expect(validateReleaseManifest(manifest)).toEqual(manifest);
    const repeated = await assembleOrdinaryUnsignedRelease(
      preserved,
      smoke,
      join(root, 'repeated'),
    );
    expect(repeated).toEqual(manifest);
    expect(cli('release-checksums.mjs', [output])).toContain('Checksummed all 10 uploaded assets');
    expect((await readdir(output)).sort()).toEqual(
      [
        ...manifest.assets.map(({ name }) => name),
        'release-manifest.json',
        'SHA256SUMS.txt',
      ].sort(),
    );
    await writeFile(
      join(smoke, 'windows-hosted-runtime-arm64.json'),
      JSON.stringify({ workflowRunId: '999' }),
    );
    await expect(
      assembleOrdinaryUnsignedRelease(preserved, smoke, join(root, 'wrong-run')),
    ).rejects.toThrow(/producer run/u);
    const provenancePath = join(
      preserved,
      'arm64/tmp/release-upload/provenance-win-arm64-setup.json',
    );
    const changedSource = JSON.parse(await readFile(provenancePath, 'utf8')) as Record<
      string,
      unknown
    >;
    await writeFile(
      provenancePath,
      JSON.stringify({ ...changedSource, sourceCommit: 'f'.repeat(40) }),
    );
    await expect(
      assembleOrdinaryUnsignedRelease(preserved, smoke, join(root, 'wrong-source')),
    ).rejects.toThrow(/source and installer/u);
  });

  it('accepts exact ordinary draft/public fixtures and rejects corrupt bytes or changed published identity', async () => {
    const { root, output, manifest } = await fixture();
    const firstAsset = manifest.assets[0];
    if (firstAsset === undefined) throw new Error('Fixture must contain release assets');
    cli('release-checksums.mjs', [output]);
    const downloaded = join(root, 'downloaded');
    await mkdir(downloaded);
    const assets = [];
    for (const name of (await readdir(output)).sort()) {
      const bytes = await readFile(join(output, name));
      await copyFile(join(output, name), join(downloaded, name));
      assets.push({
        id: assets.length + 1,
        name,
        size: bytes.length,
        digest: `sha256:${hash(bytes)}`,
      });
    }
    const manifestPath = join(output, 'release-manifest.json');
    const checksumsPath = join(output, 'SHA256SUMS.txt');
    const draftPath = join(root, 'draft.json');
    const publishedPath = join(root, 'published.json');
    const latestPath = join(root, 'latest.json');
    const canonicalUrl = `https://github.com/gee666/talking-quill/releases/tag/${tag}`;
    const draft = {
      id: 123,
      draft: true,
      prerelease: false,
      tag_name: tag,
      target_commitish: commit,
      html_url: canonicalUrl,
      assets,
    };
    const verifyDraft = () =>
      cli('verify-draft-release.mjs', [
        '--ordinary-unsigned',
        tag,
        commit,
        manifestPath,
        checksumsPath,
        draftPath,
        downloaded,
      ]);
    for (const html_url of [
      canonicalUrl,
      'https://github.com/gee666/talking-quill/releases/tag/untagged-abcdef123',
    ]) {
      await writeFile(draftPath, JSON.stringify({ ...draft, html_url }));
      expect(verifyDraft()).toContain('Authenticated draft release verified');
    }
    await writeFile(draftPath, JSON.stringify({ ...draft, target_commitish: 'f'.repeat(40) }));
    expect(verifyDraft).toThrow();
    await writeFile(draftPath, JSON.stringify(draft));
    await writeFile(join(downloaded, firstAsset.name), 'corrupted download');
    expect(verifyDraft).toThrow();
    await copyFile(join(output, firstAsset.name), join(downloaded, firstAsset.name));
    const published = { ...draft, draft: false };
    await writeFile(publishedPath, JSON.stringify(published));
    await writeFile(latestPath, JSON.stringify(published));
    const verifyPublic = () =>
      cli('verify-public-release.mjs', [tag, commit, manifestPath, publishedPath, latestPath]);
    expect(verifyPublic()).toContain('with 11 assets');
    for (const mutation of [
      { target_commitish: 'f'.repeat(40) },
      { tag_name: 'v0.0.72' },
      { draft: true },
      { prerelease: true },
      { html_url: 'https://example.com/release' },
      { assets: assets.slice(1) },
      { assets: [...assets, assets[0]] },
    ]) {
      await writeFile(latestPath, JSON.stringify({ ...published, ...mutation }));
      expect(verifyPublic).toThrow();
    }
    // Even matching local/remote bytes and rewritten checksums cannot override
    // the asset hashes sealed into the assembled manifest.
    await writeFile(join(output, firstAsset.name), 'tampered installer');
    const checksumLines = [];
    for (const name of (await readdir(output)).filter((name) => name !== 'SHA256SUMS.txt').sort()) {
      checksumLines.push(`${hash(await readFile(join(output, name)))}  ${name}`);
    }
    await writeFile(checksumsPath, `${checksumLines.join('\n')}\n`);
    const tamperedAssets = [];
    for (const asset of assets) {
      const bytes = await readFile(join(output, asset.name));
      await copyFile(join(output, asset.name), join(downloaded, asset.name));
      tamperedAssets.push({ ...asset, size: bytes.length, digest: `sha256:${hash(bytes)}` });
    }
    await writeFile(draftPath, JSON.stringify({ ...draft, assets: tamperedAssets }));
    expect(verifyDraft).toThrow();
  });
});
