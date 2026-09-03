import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';
import {
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_REQUEST_SCHEDULE,
} from '../../scripts/windows-installed-acceptance-schedule.mjs';

const roots: string[] = [];
const sha = (byte: string) => byte.repeat(64);

async function run(producerIdentity: string) {
  const root = await mkdtemp(resolve('tmp/gate-binding-test-'));
  roots.push(root);
  const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const publicKey = keys.publicKey.export({ format: 'der', type: 'spki' }).toString('base64url');
  const sourceCommit = 'a'.repeat(40);
  const sourceTree = 'b'.repeat(40);
  const acceptedIdentity = sha('1');
  const buildId = sha('2');
  const payload = {
    version: 1,
    purpose: 'talking-quill/windows-installed-acceptance-producer-result',
    sourceCommit,
    sourceTree,
    buildId,
    producerArtifactSetIdentity: producerIdentity,
  };
  const envelope = {
    payload,
    signatureBase64url: sign('sha256', Buffer.from(canonicalAcceptanceJson(payload)), {
      key: keys.privateKey,
      dsaEncoding: 'ieee-p1363',
    }).toString('base64url'),
  };
  const producerBytes = Buffer.from(`${canonicalAcceptanceJson(envelope)}\n`);
  const producerPath = resolve(root, 'producer-result.json');
  await writeFile(producerPath, producerBytes);
  const evidence = {
    schemaVersion: 2,
    result: 'passed',
    architecture: 'x64',
    sourceCommit,
    sourceTree,
    buildId,
    bundleSha256: sha('3'),
    bundleManifestSha256: sha('4'),
    bundleAuthorizationSha256: sha('5'),
    producerArtifactSetIdentity: acceptedIdentity,
    producerResult: {
      sha256: createHash('sha256').update(producerBytes).digest('hex'),
      requestPublicKeySpkiBase64url: publicKey,
    },
    candidateInstallerSha256: sha('6'),
    targetIdentity: {
      releaseBuildDigest: sha('7'),
      packageLayoutDigest: sha('8'),
      gatewaySha256: sha('9'),
      ownerSha256: sha('a'),
    },
    acceptanceNative: Object.fromEntries(
      ['broker', 'bootstrap', 'launcher'].map((name) => [name, { sha256: sha('b'), bytes: 1 }]),
    ),
    preflight: {
      manifestSignatureVerified: true,
      requestSignaturesVerified: ACCEPTANCE_REQUEST_SCHEDULE.length,
      validationKeySha256: sha('c'),
      faultRecordsVerified: 10,
      faultChainHeadSha256: sha('d'),
    },
    phases: ACCEPTANCE_MATRIX.map((phase) => ({
      phase,
      result: {
        result: 'passed',
        ...(phase === 'manual-physical-observation' ? { physicalObservation: true } : {}),
        ...(phase === 'residue' ? { zeroResidue: true } : {}),
      },
    })),
    artifacts: { candidate: { installer: { sha256: sha('6') } } },
  };
  const evidencePath = resolve(root, 'evidence.json');
  await writeFile(evidencePath, JSON.stringify(evidence));
  const outputPath = resolve(root, 'gate.json');
  const child = spawnSync(
    process.execPath,
    [
      'scripts/verify-windows-installed-acceptance-evidence.mjs',
      '--evidence',
      evidencePath,
      '--producer-result',
      producerPath,
      '--output',
      outputPath,
      '--architecture',
      'x64',
      '--source-commit',
      sourceCommit,
      '--source-tree',
      sourceTree,
      '--bundle-sha256',
      sha('3'),
      '--manifest-sha256',
      sha('4'),
      '--authorization-sha256',
      sha('5'),
      '--producer-artifact-set-identity',
      acceptedIdentity,
      '--run-id',
      '1',
    ],
    { encoding: 'utf8' },
  );
  return { process: child, outputPath };
}

describe('installed acceptance exact producer gate binding', () => {
  afterEach(async () => {
    await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
  });

  it('accepts the signed producer result from the executed artifact set', async () => {
    const result = await run(sha('1'));
    expect(result.process.status, result.process.stderr).toBe(0);
    const gate = JSON.parse(await readFile(result.outputPath, 'utf8')) as {
      producerArtifactSetIdentity?: unknown;
    };
    expect(gate.producerArtifactSetIdentity).toBe(sha('1'));
  });

  it('rejects the former arbitrary 00hash producer result', async () => {
    const result = await run('0'.repeat(64));
    expect(result.process.status).not.toBe(0);
    expect(result.process.stderr).toContain('producer result binding is invalid');
  });

  it('rejects a validly signed producer result from another bundle', async () => {
    const result = await run(sha('e'));
    expect(result.process.status).not.toBe(0);
    expect(result.process.stderr).toContain('producer result binding is invalid');
  });
});
