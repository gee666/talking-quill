import { createHash } from 'node:crypto';
import { mkdir, rm, symlink, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  inspectExactArtifact,
  launchExactArtifact,
  snapshotExactArtifact,
  verifyExactArtifact,
} from '../../scripts/exact-artifact-harness.mjs';

const root = resolve('tmp/tests/exact-artifact-harness');
const outside = resolve('tmp/tests/exact-artifact-outside.bin');
const outsideRoot = resolve('tmp/tests/exact-artifact-outside-root');
const snapshots = resolve('tmp/tests/exact-snapshots');

afterEach(async () => {
  await Promise.all([
    rm(root, { recursive: true, force: true }),
    rm(outside, { force: true }),
    rm(outsideRoot, { recursive: true, force: true }),
    rm(snapshots, { recursive: true, force: true }),
  ]);
});

describe('exact-artifact launch harness', () => {
  it('binds platform suites to an isolated copy of the contained executable bytes', async () => {
    await mkdir(root, { recursive: true });
    const bytes = Buffer.from('exact packaged runtime fixture');
    await writeFile(resolve(root, 'runtime.bin'), bytes);
    const sha256 = createHash('sha256').update(bytes).digest('hex');

    const artifact = await inspectExactArtifact({
      root,
      executable: 'runtime.bin',
      expectedSha256: sha256,
    });
    expect(artifact.sha256).toBe(sha256);
    expect(artifact.treeSha256).toMatch(/^[a-f0-9]{64}$/u);
    await expect(verifyExactArtifact(artifact)).resolves.toMatchObject({ sha256 });
    const snapshot = await snapshotExactArtifact(artifact, snapshots);
    expect(snapshot).toMatchObject({ sha256, treeSha256: artifact.treeSha256 });
    await expect(
      inspectExactArtifact({ root, executable: 'runtime.bin', expectedSha256: '0'.repeat(64) }),
    ).rejects.toThrow(/SHA-256 mismatch/u);

    await writeFile(resolve(root, 'runtime.bin'), 'replaced after inspection');
    await expect(verifyExactArtifact(artifact)).rejects.toThrow(/SHA-256 mismatch/u);
    await expect(verifyExactArtifact(snapshot)).resolves.toMatchObject({ sha256 });
  });

  it('rejects executable substitution outside the declared artifact root', async () => {
    await mkdir(root, { recursive: true });
    await writeFile(outside, 'outside');
    await expect(inspectExactArtifact({ root, executable: outside })).rejects.toThrow(
      /must be contained/u,
    );
    await expect(
      launchExactArtifact(
        { root, executable: outside, sha256: '0'.repeat(64), treeSha256: '0'.repeat(64) },
        [],
      ),
    ).rejects.toThrow(/isolated launch snapshot/u);
  });

  it('rejects symlinks whose runtime bytes escape the artifact root', async () => {
    await Promise.all([mkdir(root, { recursive: true }), mkdir(outsideRoot, { recursive: true })]);
    await writeFile(resolve(root, 'runtime.bin'), 'runtime');
    await writeFile(resolve(outsideRoot, 'mutable.bin'), 'outside runtime bytes');
    await symlink(outsideRoot, resolve(root, 'escape'), 'junction');

    await expect(inspectExactArtifact({ root, executable: 'runtime.bin' })).rejects.toThrow(
      /symlink escapes/u,
    );
  });
});
