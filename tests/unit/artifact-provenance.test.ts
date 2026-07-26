import { mkdir, readFile, rename, rm, symlink, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  artifactProvenanceManifestPath,
  artifactUploadPaths,
  validateArtifactProvenanceManifest,
  verifyArtifactProvenanceManifest,
  writeArtifactProvenanceManifest,
} from '../../scripts/artifact-provenance.mjs';

const root = resolve('tmp', 'artifact-provenance-fixtures');
const packageRoot = resolve(root, 'release', 'win-unpacked');
const artifact = resolve(root, 'release', 'Talking-Quill-1.0.2-win-x64.exe');

beforeEach(async () => {
  await rm(root, { recursive: true, force: true });
  await rm(artifactProvenanceManifestPath, { force: true });
  await mkdir(resolve(packageRoot, 'resources'), { recursive: true });
  await writeFile(resolve(packageRoot, 'Talking Quill.exe'), Buffer.from('application-v1'));
  await writeFile(resolve(packageRoot, 'resources', 'app.asar'), Buffer.from('asar-v1'));
  await writeFile(artifact, Buffer.from('installer-v1'));
});

afterEach(async () => {
  await rm(root, { recursive: true, force: true });
  await rm(artifactProvenanceManifestPath, { force: true });
});

async function writeManifest() {
  return writeArtifactProvenanceManifest({
    version: '1.0.2',
    platform: 'win',
    arch: 'x64',
    packageRoot,
    artifacts: [artifact],
  });
}

describe('canonical artifact provenance manifest', () => {
  it('binds the canonical package inventory and final artifact bytes for upload', async () => {
    const manifest = await writeManifest();
    await expect(verifyArtifactProvenanceManifest()).resolves.toEqual(manifest);
    expect(artifactUploadPaths(manifest)).toEqual([
      'tmp/artifact-provenance-fixtures/release/win-unpacked/**',
      'tmp/artifact-provenance-fixtures/release/Talking-Quill-1.0.2-win-x64.exe',
      'artifact-provenance.json',
    ]);
    const persisted = await readFile(artifactProvenanceManifestPath, 'utf8');
    expect(persisted.endsWith('\n')).toBe(true);
    expect(manifest.entries).toHaveLength(3);
    expect(manifest.entries.every((entry) => !entry.path.includes('\\'))).toBe(true);
  });

  it('rejects stale source provenance rather than accepting a prior manifest', async () => {
    await writeManifest();
    const manifest = JSON.parse(await readFile(artifactProvenanceManifestPath, 'utf8')) as {
      sourceCommit: string;
    };
    manifest.sourceCommit = '0'.repeat(40);
    await writeFile(artifactProvenanceManifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'stale for the current source commit',
    );
  });

  it('rejects stale or substituted source-tree provenance', async () => {
    await writeManifest();
    const manifest = JSON.parse(await readFile(artifactProvenanceManifestPath, 'utf8')) as {
      sourceTreeSha256: string;
    };
    manifest.sourceTreeSha256 = '0'.repeat(64);
    await writeFile(artifactProvenanceManifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'stale for the current source tree',
    );
  });

  it('rejects absolute or repository-escaping manifest paths', async () => {
    const manifest = await writeManifest();
    const absolute = {
      ...structuredClone(manifest),
      package: {
        ...manifest.package,
        root: process.platform === 'win32' ? 'C:/outside' : '/outside',
      },
    };
    expect(() => validateArtifactProvenanceManifest(absolute)).toThrow(
      'escapes the canonical repository root',
    );

    const escaping = {
      ...structuredClone(manifest),
      entries: manifest.entries.map((entry, index) =>
        index === 0 ? { ...entry, path: '../outside' } : entry,
      ),
    };
    expect(() => validateArtifactProvenanceManifest(escaping)).toThrow(
      'escapes the canonical repository root',
    );
  });

  it('rejects a package root replaced by a filesystem redirect after inspection', async () => {
    await writeManifest();
    const original = resolve(root, 'original-package');
    await rename(packageRoot, original);
    await symlink(original, packageRoot, process.platform === 'win32' ? 'junction' : 'dir');
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'package root is not a canonical physical path',
    );
  });

  it('rejects a manifest substituted from a different package identity', async () => {
    await writeManifest();
    process.env.TALKING_QUILL_PACKAGE_TARGET = 'win';
    process.env.TALKING_QUILL_PACKAGE_ARCH = 'arm64';
    process.env.TALKING_QUILL_PACKAGE_ROOT =
      'tmp/artifact-provenance-fixtures/release/win-unpacked';
    try {
      await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
        'does not match the requested package identity',
      );
    } finally {
      delete process.env.TALKING_QUILL_PACKAGE_TARGET;
      delete process.env.TALKING_QUILL_PACKAGE_ARCH;
      delete process.env.TALKING_QUILL_PACKAGE_ROOT;
    }
  });

  it('detects package-byte substitution after inspection', async () => {
    await writeManifest();
    await writeFile(resolve(packageRoot, 'resources', 'app.asar'), Buffer.from('substituted'));
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'does not match current package bytes or inventory',
    );
  });

  it('detects final-artifact substitution and new unmanifested package files', async () => {
    await writeManifest();
    await writeFile(artifact, Buffer.from('replacement installer'));
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'does not match current package bytes or inventory',
    );

    await writeManifest();
    await writeFile(resolve(packageRoot, 'resources', 'late-added.dll'), Buffer.from('late'));
    await expect(verifyArtifactProvenanceManifest()).rejects.toThrow(
      'does not match current package bytes or inventory',
    );
  });
});
