import { createHash, randomBytes } from 'node:crypto';
import { mkdir, readFile, rm, symlink, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  createDeterministicAcceptanceZip,
  extractVerifiedAcceptanceBundle,
  verifyAcceptanceBundleArchive,
  verifyAcceptanceBundleTree,
} from '../../scripts/windows-installed-acceptance-bundle.mjs';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';

const roots: string[] = [];
const sourceCommit = '11'.repeat(20);
const sourceTree = '22'.repeat(20);
const expected = { architecture: 'x64' as const, sourceCommit, sourceTree };

function hash(bytes: Buffer): string {
  return createHash('sha256').update(bytes).digest('hex');
}

async function fixture(): Promise<string> {
  const root = resolve('tmp', `acceptance-bundle-${randomBytes(8).toString('hex')}`);
  roots.push(root);
  await mkdir(resolve(root, 'payload'), { recursive: true });
  const payloads = new Map([
    ['payload/build.txt', Buffer.from('build')],
    ['payload/requests.json', Buffer.from('{}\n')],
    ['payload/sender.exe', Buffer.from('sender')],
    ['payload/broker.exe', Buffer.from('broker')],
    ['payload/bootstrap.exe', Buffer.from('bootstrap')],
    ['payload/launcher.exe', Buffer.from('launcher')],
  ]);
  const evidence = {
    architecture: 'x64',
    outputPath: '../evidence.json',
    artifacts: {},
    acceptance: {
      sourceRevision: sourceCommit.slice(0, 12),
      buildManifestPath: 'payload/build.txt',
      signedRequestsPath: 'payload/requests.json',
      syntheticSenderPath: 'payload/sender.exe',
      acceptanceBrokerPath: 'payload/broker.exe',
      acceptanceBootstrapPath: 'payload/bootstrap.exe',
      trustedLauncherPath: 'payload/launcher.exe',
    },
  };
  payloads.set('evidence-input.json', Buffer.from(`${canonicalAcceptanceJson(evidence)}\n`));
  for (const [path, bytes] of payloads) await writeFile(resolve(root, ...path.split('/')), bytes);
  const entries = [...payloads]
    .map(([path, bytes]) => ({ path, bytes: bytes.length, sha256: hash(bytes) }))
    .sort((left, right) => Buffer.from(left.path).compare(Buffer.from(right.path)));
  const manifest = {
    schemaVersion: 1,
    classification: 'nonpromotable-installed-acceptance-kit',
    architecture: 'x64',
    sourceCommit,
    sourceTree,
    producerArtifactSetIdentity: 'ab'.repeat(32),
    entries,
  };
  await writeFile(resolve(root, 'bundle-manifest.json'), `${canonicalAcceptanceJson(manifest)}\n`);
  return root;
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe('Windows installed-acceptance bundle', () => {
  it('creates byte-identical normalized ZIPs and self-verifies extraction', async () => {
    const root = await fixture();
    const first = `${root}-one.zip`;
    const second = `${root}-two.zip`;
    roots.push(first, second);
    const one = await createDeterministicAcceptanceZip(root, first, expected);
    const two = await createDeterministicAcceptanceZip(root, second, expected);
    expect(await readFile(first)).toEqual(await readFile(second));
    expect(one.sha256).toBe(two.sha256);
    const extracted = `${root}-extracted`;
    roots.push(extracted);
    await extractVerifiedAcceptanceBundle(first, extracted, expected);
    await expect(verifyAcceptanceBundleTree(extracted, expected)).resolves.toMatchObject({
      manifest: { sourceCommit, sourceTree },
    });
    await expect(
      verifyAcceptanceBundleTree(extracted, {
        ...expected,
        manifestSha256: '00'.repeat(32),
      }),
    ).rejects.toThrow('does not match authorized extraction');
  });

  it('rejects archive tampering and extracted-tree mutation', async () => {
    const root = await fixture();
    const archive = `${root}.zip`;
    roots.push(archive);
    await createDeterministicAcceptanceZip(root, archive, expected);
    await expect(
      verifyAcceptanceBundleArchive(archive, { ...expected, bundleSha256: '00'.repeat(32) }),
    ).rejects.toThrow('does not match authorization');
    const tampered = await readFile(archive);
    tampered[40] = (tampered[40] ?? 0) ^ 1;
    await writeFile(archive, tampered);
    await expect(verifyAcceptanceBundleArchive(archive, expected)).rejects.toThrow();

    await writeFile(resolve(root, 'payload/sender.exe'), 'changed');
    await expect(verifyAcceptanceBundleTree(root, expected)).rejects.toThrow(
      'file identity mismatch',
    );
  });

  it('rejects missing, extra, case-colliding, and linked files', async () => {
    const root = await fixture();
    await writeFile(resolve(root, 'extra.txt'), 'extra');
    await expect(verifyAcceptanceBundleTree(root, expected)).rejects.toThrow('missing or extra');
    await rm(resolve(root, 'extra.txt'));

    const manifestPath = resolve(root, 'bundle-manifest.json');
    const manifest = JSON.parse(await readFile(manifestPath, 'utf8')) as {
      entries: { path: string; bytes: number; sha256: string }[];
    };
    const first = manifest.entries[0];
    if (first === undefined) throw new Error('Manifest fixture entry is missing');
    manifest.entries.splice(1, 0, { ...first, path: 'EVIDENCE-INPUT.JSON' });
    await writeFile(manifestPath, `${canonicalAcceptanceJson(manifest)}\n`);
    await expect(verifyAcceptanceBundleTree(root, expected)).rejects.toThrow('collide');

    const linked = await fixture();
    try {
      await symlink(resolve(linked, 'payload/sender.exe'), resolve(linked, 'payload/link.exe'));
      await expect(verifyAcceptanceBundleTree(linked, expected)).rejects.toThrow('link');
    } catch (error) {
      if ((error as NodeJS.ErrnoException).code !== 'EPERM') throw error;
    }
  });
});
