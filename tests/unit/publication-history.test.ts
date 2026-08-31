import { generateKeyPairSync } from 'node:crypto';
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { verifyPublicationHistory } from '../../scripts/publication-history.mjs';
import { createPublicationManifest } from '../../scripts/publication-manifest.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

async function fixture() {
  const root = await createTestDirectory('publication-history');
  roots.push(root);
  const pair = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const jwk = pair.publicKey.export({ format: 'jwk' });
  if (jwk.x === undefined || jwk.y === undefined) throw new Error('Invalid test key');
  const privateKeyPkcs8Base64 = pair.privateKey
    .export({ format: 'der', type: 'pkcs8' })
    .toString('base64');
  const publicKeyPath = join(root, 'publication.sec1');
  await writeFile(
    publicKeyPath,
    `${Buffer.concat([
      Buffer.from([4]),
      Buffer.from(jwk.x, 'base64url'),
      Buffer.from(jwk.y, 'base64url'),
    ]).toString('hex')}\n`,
  );
  const repository = 'owner/repository';
  async function manifest(name: string, tag: string, sequence: number, workflowRunId: string) {
    const directory = join(root, name);
    await mkdir(directory);
    await writeFile(join(directory, 'release-manifest.json'), '{}\n');
    await writeFile(join(directory, 'windows-promotion-lifecycle-evidence-v1.json'), '{}\n');
    const output = join(directory, 'release-publication-manifest-v1.json');
    await createPublicationManifest({
      directory,
      output,
      repository,
      tag,
      sequence,
      workflowRunId,
      sourceCommit: 'a'.repeat(40),
      sourceTree: 'b'.repeat(40),
      privateKeyPkcs8Base64,
      publicKeyPath,
    });
    return output;
  }
  const candidatePath = await manifest('candidate', 'v0.0.69', 17, '117');
  const historicalPath = await manifest('historical', 'v0.0.68', 16, '116');
  const historyIndexPath = join(root, 'index.json');
  const writeIndex = async (records: unknown[]) =>
    writeFile(historyIndexPath, `${JSON.stringify(records)}\n`);
  await writeIndex([
    {
      id: 1,
      tag: 'v0.0.68',
      draft: false,
      immutable: true,
      manifestPath: historicalPath,
    },
  ]);
  return {
    root,
    repository,
    publicKeyPath,
    candidatePath,
    historicalPath,
    historyIndexPath,
    writeIndex,
    manifest,
  };
}

const options = (value: Awaited<ReturnType<typeof fixture>>) => ({
  candidatePath: value.candidatePath,
  historyIndexPath: value.historyIndexPath,
  repository: value.repository,
  publicKeyPath: value.publicKeyPath,
  expectedCandidateTag: 'v0.0.69',
  expectedCandidateSequence: 17,
});

describe('immutable publication anti-rollback', () => {
  it('accepts only a candidate newer in both sequence and semantic version', async () => {
    const value = await fixture();
    await expect(verifyPublicationHistory(options(value))).resolves.toEqual({
      highestSequence: 16,
      accepted: 1,
    });
    const equalSequence = await value.manifest('equal-sequence', 'v0.0.70', 16, '118');
    await expect(
      verifyPublicationHistory({
        ...options(value),
        candidatePath: equalSequence,
        expectedCandidateTag: 'v0.0.70',
        expectedCandidateSequence: 16,
      }),
    ).rejects.toThrow(/sequence|identity/u);
    const lowerVersion = await value.manifest('lower-version', 'v0.0.67', 18, '119');
    await expect(
      verifyPublicationHistory({
        ...options(value),
        candidatePath: lowerVersion,
        expectedCandidateTag: 'v0.0.67',
        expectedCandidateSequence: 18,
      }),
    ).rejects.toThrow(/semantic version/u);
  });

  it('rejects sequence, run, signature, and release-container replay', async () => {
    const value = await fixture();
    const duplicate = await value.manifest('duplicate', 'v0.0.66', 16, '115');
    await value.writeIndex([
      { id: 1, tag: 'v0.0.68', draft: false, immutable: true, manifestPath: value.historicalPath },
      { id: 2, tag: 'v0.0.66', draft: false, immutable: true, manifestPath: duplicate },
    ]);
    await expect(verifyPublicationHistory(options(value))).rejects.toThrow(/duplicated/u);

    await value.writeIndex([
      { id: 1, tag: 'v9.9.9', draft: false, immutable: true, manifestPath: value.historicalPath },
    ]);
    await expect(verifyPublicationHistory(options(value))).rejects.toThrow(/container/u);

    const envelope = JSON.parse(await readFile(value.historicalPath, 'utf8')) as {
      signature: { value: string };
    };
    envelope.signature.value = `${envelope.signature.value.startsWith('A') ? 'B' : 'A'}${envelope.signature.value.slice(1)}`;
    await writeFile(value.historicalPath, `${JSON.stringify(envelope)}\n`);
    await value.writeIndex([
      { id: 1, tag: 'v0.0.68', draft: false, immutable: true, manifestPath: value.historicalPath },
    ]);
    await expect(verifyPublicationHistory(options(value))).rejects.toThrow(/signature/u);
  });
});
