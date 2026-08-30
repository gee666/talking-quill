import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { beforeEach, describe, expect, it } from 'vitest';
import { currentSourceTreeHash } from '../../scripts/artifact-provenance.mjs';
import { sealReleaseManifest } from '../../scripts/release-manifest.mjs';

const root = resolve('.');
const fixture = resolve(root, 'tmp/release-tooling-focused');
const downloaded = resolve(fixture, 'downloaded');
const commit = execFileSync('git', ['rev-parse', 'HEAD^{commit}'], {
  cwd: root,
  encoding: 'utf8',
}).trim();
const sourceTreeSha256 = await currentSourceTreeHash();

beforeEach(() => {
  rmSync(fixture, { recursive: true, force: true });
  mkdirSync(downloaded, { recursive: true });
});

describe('active release tooling', () => {
  it('verifies exact draft identity, inventory, checksums, and downloaded bytes', () => {
    const artifact = 'Talking-Quill-1.0.0-win-x64.exe';
    writeFileSync(resolve(fixture, artifact), 'installer bytes');
    const manifest = sealReleaseManifest({
      schemaVersion: 1,
      repository: 'gee666/talking-quill',
      tag: 'v1.0.0',
      version: '1.0.0',
      sourceCommit: commit,
      platform: 'win',
      architecture: 'x64',
      promotable: true,
      workflowRunId: null,
      generatedAt: null,
      provenance: [
        {
          name: 'provenance-win-x64.json',
          platform: 'win',
          arch: 'x64',
          sourceTreeSha256: 'a'.repeat(64),
        },
      ],
      assets: [{ name: artifact, bytes: 15, sha256: sha256(resolve(fixture, artifact)) }],
    });
    const manifestPath = resolve(fixture, 'release-manifest.json');
    writeFileSync(manifestPath, `${JSON.stringify(manifest)}\n`);
    const checksumsPath = resolve(fixture, 'SHA256SUMS.txt');
    writeFileSync(
      checksumsPath,
      `${sha256(resolve(fixture, artifact))}  ${artifact}\n${sha256(manifestPath)}  release-manifest.json\n`,
    );
    const names = [artifact, 'release-manifest.json', 'SHA256SUMS.txt'];
    for (const name of names) cpSync(resolve(fixture, name), resolve(downloaded, name));
    const responsePath = resolve(fixture, 'draft-response.json');
    const response = {
      draft: true,
      prerelease: false,
      tag_name: 'v1.0.0',
      target_commitish: commit,
      html_url: 'https://github.com/gee666/talking-quill/releases/tag/untagged-0123456789abcdef',
      assets: names.map((name) => ({
        name,
        size: readFileSync(resolve(fixture, name)).length,
        digest: `sha256:${sha256(resolve(fixture, name))}`,
      })),
    };
    writeFileSync(responsePath, JSON.stringify(response));
    const args = ['v1.0.0', commit, manifestPath, checksumsPath, responsePath, downloaded];

    expect(runVerifyDraft(args)).toContain('Authenticated draft release verified.');
    writeFileSync(manifestPath, `${JSON.stringify({ ...manifest, promotable: false })}\n`);
    expect(() => runVerifyDraft(args)).toThrow();
    writeFileSync(manifestPath, `${JSON.stringify(manifest)}\n`);
    writeFileSync(resolve(downloaded, artifact), 'changed bytes');
    expect(() => runVerifyDraft(args)).toThrow();
  });

  it('assembles the canonical Windows x64 identity and provenance set', () => {
    createAssemblyFixture();
    expect(
      runAssemble({
        TALKING_QUILL_RELEASE_COMMIT: commit,
        TALKING_QUILL_DETERMINISTIC: '1',
      }),
    ).toContain('6 validated Windows x64 inputs plus release-manifest.json');
    const manifest = JSON.parse(
      readFileSync(resolve(fixture, 'release-manifest.json'), 'utf8'),
    ) as {
      assets: { name: string }[];
      promotable: boolean;
      provenance: { platform: string; arch: string }[];
    };
    expect(manifest.promotable).toBe(true);
    expect(manifest.assets).toHaveLength(6);
    expect(manifest.provenance.map(({ platform, arch }) => `${platform}-${arch}`)).toEqual([
      'win-x64',
    ]);
    expect(manifest.assets.map(({ name }) => name)).not.toContain('smoke-evidence-win-x64.json');
  });

  it('rejects stale source-tree provenance before producing a manifest', () => {
    createAssemblyFixture();
    const path = resolve(fixture, 'provenance-win-x64.json');
    const provenance = JSON.parse(readFileSync(path, 'utf8')) as {
      sourceTreeSha256: string;
    };
    provenance.sourceTreeSha256 = '0'.repeat(64);
    writeFileSync(path, JSON.stringify(provenance));

    expect(() =>
      runAssemble({
        TALKING_QUILL_RELEASE_COMMIT: commit,
        TALKING_QUILL_DETERMINISTIC: '1',
      }),
    ).toThrow('Windows x64 provenance identity mismatch');
  });

  it('rejects malformed current provenance before producing a manifest', () => {
    createAssemblyFixture();
    const path = resolve(fixture, 'provenance-win-x64.json');
    const provenance = JSON.parse(readFileSync(path, 'utf8')) as {
      sourceTreeSha256?: string;
    };
    delete provenance.sourceTreeSha256;
    writeFileSync(path, JSON.stringify(provenance));

    expect(() =>
      runAssemble({
        TALKING_QUILL_RELEASE_COMMIT: commit,
        TALKING_QUILL_DETERMINISTIC: '1',
      }),
    ).toThrow('Provenance schema mismatch: provenance-win-x64.json');
  });

  it('keeps assembly commit-bound and exact-input allowlisted', () => {
    expect(() => runAssemble({ TALKING_QUILL_RELEASE_COMMIT: 'b'.repeat(40) })).toThrow(
      'Release source commit is invalid or does not match the checkout',
    );
    expect(() => runAssemble({ TALKING_QUILL_RELEASE_COMMIT: commit })).toThrow(
      'Release input allowlist mismatch',
    );
  });
});

function createAssemblyFixture(): void {
  rmSync(fixture, { recursive: true, force: true });
  mkdirSync(fixture, { recursive: true });

  const installer = 'Talking-Quill-1.0.0-win-x64.exe';
  writeFileSync(resolve(fixture, installer), `${installer} bytes`);
  const blockmap = `${installer}.blockmap`;
  writeFileSync(resolve(fixture, blockmap), `${blockmap} bytes`);
  const entries = [
    {
      role: 'final-artifact',
      path: `release/${installer}`,
      kind: 'file',
      size: readFileSync(resolve(fixture, installer)).length,
      sha256: sha256(resolve(fixture, installer)),
    },
  ];
  writeFileSync(
    resolve(fixture, 'provenance-win-x64.json'),
    JSON.stringify({
      schemaVersion: 1,
      sourceCommit: commit,
      sourceTreeSha256,
      package: {
        version: '1.0.0',
        platform: 'win',
        arch: 'x64',
        root: 'release/win-x64-unpacked',
      },
      entries,
    }),
  );

  const roles = ['gateway', 'owner'].map((role, index) => ({
    role,
    path: `roles/${role}-x64`,
    sha256: String(index + 1).repeat(64),
    suppressionCapable: role === 'owner',
  }));
  const identity = {
    schemaVersion: 1,
    version: '1.0.0',
    platform: 'win',
    architecture: 'x64',
    packageSha256: sha256(resolve(fixture, installer)),
    transactionBinding: 'source-target-package-sha256-v1',
    authorization: {
      scheme: 'p256-sha256-v1',
      verificationKeySha256: '66'.repeat(32),
      signature: 'MEUCIQfixture==',
    },
    releaseBuildDigest: 'b'.repeat(64),
    packageLayoutDigest: 'c'.repeat(64),
    predecessor: {
      platform: 'win',
      architecture: 'x64',
      version: '0.9.0',
      releaseBuildDigest: 'd'.repeat(64),
      gatewaySha256: 'e'.repeat(64),
      ownerSha256: 'f'.repeat(64),
    },
    roles,
  };
  writeFileSync(resolve(fixture, 'release-identity-win-x64.json'), JSON.stringify(identity));
  writeFileSync(
    resolve(fixture, 'latest-x64.yml'),
    `talkingQuillRelease: ${JSON.stringify(identity)}\n`,
  );
  writeFileSync(resolve(fixture, 'THIRD_PARTY_NOTICES.txt'), 'controlled notices fixture');
}

function runVerifyDraft(args: string[]): string {
  return execFileSync(process.execPath, ['scripts/verify-draft-release.mjs', ...args], {
    cwd: root,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

function runAssemble(environment: NodeJS.ProcessEnv): string {
  return execFileSync(process.execPath, ['scripts/assemble-release.mjs', 'v1.0.0', fixture], {
    cwd: root,
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

function sha256(path: string): string {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
