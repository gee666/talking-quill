import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFileSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';

const root = resolve('.');
const fixture = resolve(root, 'tmp/release-tooling-fixture');
const commit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: root, encoding: 'utf8' }).trim();
const artifacts = [
  ['win', 'x64', 'exe'],
  ['win', 'arm64', 'exe'],
  ['mac', 'x64', 'dmg'],
  ['mac', 'x64', 'zip'],
  ['mac', 'arm64', 'dmg'],
  ['mac', 'arm64', 'zip'],
] as const;

beforeEach(() => {
  rmSync(fixture, { recursive: true, force: true });
  mkdirSync(fixture, { recursive: true });
  for (const [platform, arch, extension] of artifacts) {
    const name = `Talking-Quill-1.0.0-${platform}-${arch}.${extension}`;
    writeFileSync(resolve(fixture, name), name);
  }
  for (const [platform, arch] of [
    ['win', 'x64'],
    ['win', 'arm64'],
    ['mac', 'x64'],
    ['mac', 'arm64'],
  ] as const) {
    const names = artifacts
      .filter(([p, a]) => p === platform && a === arch)
      .map(([p, a, extension]) => `Talking-Quill-1.0.0-${p}-${a}.${extension}`);
    writeFileSync(
      resolve(fixture, `provenance-${platform}-${arch}.json`),
      JSON.stringify({
        schemaVersion: 1,
        sourceCommit: commit,
        sourceTreeSha256: 'a'.repeat(64),
        package: {
          version: '1.0.0',
          platform,
          arch,
          root: `release/${platform}-${arch}-unpacked`,
        },
        entries: names.map((name) => ({
          role: 'final-artifact',
          path: `release/${name}`,
          kind: 'file',
          size: readFileSync(resolve(fixture, name)).length,
          sha256: sha256(resolve(fixture, name)),
        })),
      }),
    );
  }
  writeFileSync(resolve(fixture, 'THIRD_PARTY_NOTICES.txt'), 'controlled notices fixture');
});

describe('release assembly tooling', () => {
  it('binds six installers, four provenance records, notices, manifest, and checksums', () => {
    expect(() =>
      run('scripts/assemble-release.mjs', ['v1.0.0', fixture], {
        TALKING_QUILL_RELEASE_COMMIT: 'b'.repeat(40),
      }),
    ).toThrow();
    run('scripts/assemble-release.mjs', ['v1.0.0', fixture], {
      TALKING_QUILL_RELEASE_COMMIT: commit,
    });
    run('scripts/release-checksums.mjs', [fixture]);
    const manifest = JSON.parse(
      readFileSync(resolve(fixture, 'release-manifest.json'), 'utf8'),
    ) as { assets: { name: string }[] };
    expect(manifest.assets).toHaveLength(11);
    expect(
      readFileSync(resolve(fixture, 'SHA256SUMS.txt'), 'utf8').trim().split('\n'),
    ).toHaveLength(12);

    const response = {
      draft: true,
      prerelease: false,
      tag_name: 'v1.0.0',
      target_commitish: commit,
      html_url: 'https://github.com/gee666/talking-quill/releases/tag/v1.0.0',
      assets: [
        ...manifest.assets.map(({ name }) => name),
        'release-manifest.json',
        'SHA256SUMS.txt',
      ].map((name) => ({
        name,
        size: readFileSync(resolve(fixture, name)).length,
        digest: `sha256:${sha256(resolve(fixture, name))}`,
      })),
    };
    writeFileSync(resolve(fixture, 'draft-response.json'), JSON.stringify(response));
    const downloaded = resolve(fixture, 'downloaded');
    mkdirSync(downloaded);
    for (const { name } of response.assets) {
      copyFileSync(resolve(fixture, name), resolve(downloaded, name));
    }
    const verifyArguments = [
      'v1.0.0',
      commit,
      resolve(fixture, 'release-manifest.json'),
      resolve(fixture, 'SHA256SUMS.txt'),
      resolve(fixture, 'draft-response.json'),
      downloaded,
    ];
    run('scripts/verify-draft-release.mjs', verifyArguments);
    const manifestPath = resolve(fixture, 'release-manifest.json');
    const wrongManifest = JSON.parse(readFileSync(manifestPath, 'utf8')) as {
      sourceCommit: string;
    };
    wrongManifest.sourceCommit = 'b'.repeat(40);
    writeFileSync(manifestPath, JSON.stringify(wrongManifest));
    expect(() => run('scripts/verify-draft-release.mjs', verifyArguments)).toThrow();
  });

  it('rejects provenance that omits canonical source-tree or file evidence', () => {
    for (const name of [
      'provenance-win-x64.json',
      'provenance-win-arm64.json',
      'provenance-mac-x64.json',
      'provenance-mac-arm64.json',
    ]) {
      const path = resolve(fixture, name);
      const provenance = JSON.parse(readFileSync(path, 'utf8')) as {
        sourceTreeSha256?: string;
      };
      delete provenance.sourceTreeSha256;
      writeFileSync(path, JSON.stringify(provenance));
    }
    expect(() =>
      run('scripts/assemble-release.mjs', ['v1.0.0', fixture], {
        TALKING_QUILL_RELEASE_COMMIT: commit,
      }),
    ).toThrow('Provenance schema mismatch');
  });

  it('rejects unexpected assets, wrong repositories, and public-latest draft confusion', () => {
    writeFileSync(resolve(fixture, 'unexpected.exe'), 'unexpected');
    expect(() =>
      run('scripts/assemble-release.mjs', ['v1.0.0', fixture], {
        TALKING_QUILL_RELEASE_COMMIT: commit,
      }),
    ).toThrow();
    const result = spawnSync(
      process.execPath,
      ['scripts/verify-release-tag.mjs', 'v0.0.1', '--dry-run'],
      {
        cwd: root,
        env: { ...process.env, GITHUB_REPOSITORY: 'gee666/legacy-origin' },
        encoding: 'utf8',
      },
    );
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('restricted to gee666/talking-quill');
  });
});

function run(script: string, args: string[], environment: NodeJS.ProcessEnv = {}): string {
  return execFileSync(process.execPath, [script, ...args], {
    cwd: root,
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}
function sha256(path: string): string {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
