import { generateKeyPairSync } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  createPublicationManifest,
  verifyPublicationManifest,
} from '../../scripts/publication-manifest.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

function keyPair() {
  const value = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const jwk = value.publicKey.export({ format: 'jwk' });
  if (jwk.x === undefined || jwk.y === undefined) throw new Error('Invalid test key');
  return {
    privateKeyPkcs8Base64: value.privateKey
      .export({ format: 'der', type: 'pkcs8' })
      .toString('base64'),
    publicSec1: Buffer.concat([
      Buffer.from([4]),
      Buffer.from(jwk.x, 'base64url'),
      Buffer.from(jwk.y, 'base64url'),
    ]).toString('hex'),
  };
}

async function fixture() {
  const root = await createTestDirectory('publication-manifest');
  roots.push(root);
  const directory = join(root, 'artifacts');
  await mkdir(directory);
  await writeFile(join(directory, 'release-manifest.json'), '{"release":true}\n');
  await writeFile(
    join(directory, 'windows-promotion-lifecycle-evidence-v1.json'),
    '{"passed":true}\n',
  );
  await writeFile(join(directory, 'artifact.exe'), 'exact artifact bytes');
  const key = keyPair();
  const publicKeyPath = join(root, 'publication.sec1');
  await writeFile(publicKeyPath, `${key.publicSec1}\n`);
  return {
    directory,
    output: join(directory, 'release-publication-manifest-v1.json'),
    publicKeyPath,
    ...key,
    repository: 'owner/repository',
    tag: 'v0.0.69',
    sequence: '17',
    workflowRunId: '123',
    sourceCommit: 'a'.repeat(40),
    sourceTree: 'b'.repeat(40),
  };
}

describe('signed publication manifest', () => {
  it('binds source, sequence, evidence, and every logical asset to content objects', async () => {
    const value = await fixture();
    const envelope = await createPublicationManifest({ ...value, output: value.output });
    expect(
      envelope.payload.objects.every(({ objectName, sha256 }) => objectName === `sha256-${sha256}`),
    ).toBe(true);
    await expect(verifyPublicationManifest({ ...value, path: value.output })).resolves.toEqual(
      envelope,
    );
  });

  it('rejects changed bytes, sequence, and a non-pinned signing key', async () => {
    const value = await fixture();
    await createPublicationManifest({ ...value, output: value.output });
    await writeFile(join(value.directory, 'artifact.exe'), 'changed');
    await expect(verifyPublicationManifest({ ...value, path: value.output })).rejects.toThrow(
      /inventory changed/u,
    );
    await expect(
      verifyPublicationManifest({ ...value, path: value.output, sequence: '18' }),
    ).rejects.toThrow(/context/u);
    const unrelated = keyPair();
    await expect(
      createPublicationManifest({
        ...value,
        output: value.output,
        privateKeyPkcs8Base64: unrelated.privateKeyPkcs8Base64,
      }),
    ).rejects.toThrow(/dedicated repository pin/u);
  });
});
