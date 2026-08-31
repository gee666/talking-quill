import { generateKeyPairSync } from 'node:crypto';
import { mkdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  createWindowsPromotionEvidence,
  verifyWindowsPromotionEvidence,
} from '../../scripts/windows-promotion-evidence.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));
const sha = (byte: string) => byte.repeat(64);
const source = (byte: string) => byte.repeat(40);

function success(architecture: string, operation: 'fresh' | 'repair') {
  const controller = {
    pid: 100,
    parentPid: 10,
    sessionId: 1,
    creationUtcTicks: 1000,
    imagePath: 'C:\\setup.exe',
    imageSha256: sha('a'),
    userSid: 'S-1-5-21-1',
    logonId: '1:2',
  };
  const worker = { ...controller, pid: 101, parentPid: 100, creationUtcTicks: 1001 };
  return {
    schemaVersion: 2,
    installer: 'setup.exe',
    installerSha256: sha('a'),
    architecture,
    sourceCommit: source('b'),
    sourceTree: source('c'),
    packageMode: operation === 'fresh' ? 'fresh' : 'update',
    operation,
    targetReleaseBuildDigest: sha('d'),
    targetGatewaySha256: sha('e'),
    targetOwnerSha256: sha('f'),
    controllerPid: 100,
    authenticatedSetupPids: [100, 101],
    processIdentities: [controller, worker],
    pipeObserved: true,
    nativeAuthenticationReceipt: { transcriptSha256: sha('1') },
    installedIdentityBound: true,
    registrationsExact: true,
    terminalTopology: true,
    exitCode: 0,
    interpreterProcessStarts: [],
    observerErrors: [],
    passed: true,
    ...(operation === 'fresh'
      ? {
          productionInterruptions: {
            fresh: true,
            removeFreshRecovery: true,
            uninstall: true,
            finishUninstallRecovery: true,
          },
        }
      : {}),
  };
}

function fault(architecture: string) {
  return {
    schemaVersion: 1,
    architecture,
    sourceCommit: source('b'),
    sourceTree: source('c'),
    productionUpdateInterrupted: true,
    candidateSha256: sha('a'),
    releaseBuildDigest: sha('d'),
    faults: Array.from({ length: 10 }, (_, index) => ({
      fault: `phase-${String(index)}`,
      crashExitCode: 197,
      recoveryExitCode: 78,
      inspectedBeforeRepair: true,
      productionRecoveryInterrupted: true,
      recoveredReleaseBuildDigest: sha('d'),
    })),
    passed: true,
  };
}

async function fixture() {
  const root = await createTestDirectory('windows-promotion-evidence');
  roots.push(root);
  const directory = join(root, 'evidence');
  await mkdir(directory);
  for (const architecture of ['arm64', 'x64']) {
    await writeFile(
      join(directory, `windows-installer-success-fresh-${architecture}.json`),
      JSON.stringify(success(architecture, 'fresh')),
    );
    await writeFile(
      join(directory, `windows-installer-success-${architecture}.json`),
      JSON.stringify(success(architecture, 'repair')),
    );
    await writeFile(
      join(directory, `windows-installer-fault-recovery-${architecture}.json`),
      JSON.stringify(fault(architecture)),
    );
  }
  const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const jwk = publicKey.export({ format: 'jwk' });
  if (jwk.x === undefined || jwk.y === undefined) throw new Error('Generated P-256 key is invalid');
  const sec1 = Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x, 'base64url'),
    Buffer.from(jwk.y, 'base64url'),
  ]);
  const publicKeyPath = join(root, 'release-key.sec1');
  await writeFile(publicKeyPath, sec1.toString('hex'));
  return {
    directory,
    publicKeyPath,
    output: join(directory, 'windows-promotion-lifecycle-evidence-v1.json'),
    privateKey: privateKey.export({ format: 'der', type: 'pkcs8' }).toString('base64'),
  };
}

describe('Windows promotion lifecycle evidence', () => {
  it('binds exact lifecycle claims to the pinned protected P-256 release key', async () => {
    const value = await fixture();
    await createWindowsPromotionEvidence({
      ...value,
      repository: 'owner/repository',
      workflowRunId: '123',
      privateKeyPkcs8Base64: value.privateKey,
    });
    await expect(
      verifyWindowsPromotionEvidence({
        path: value.output,
        directory: value.directory,
        repository: 'owner/repository',
        workflowRunId: '123',
        publicKeyPath: value.publicKeyPath,
      }),
    ).resolves.toMatchObject({ payload: { promotionClass: 'protected-release-acceptance' } });

    const path = join(value.directory, 'windows-installer-success-x64.json');
    await writeFile(path, JSON.stringify({ ...success('x64', 'repair'), exitCode: 1 }));
    await expect(
      verifyWindowsPromotionEvidence({
        path: value.output,
        directory: value.directory,
        repository: 'owner/repository',
        workflowRunId: '123',
        publicKeyPath: value.publicKeyPath,
      }),
    ).rejects.toThrow(/did not pass exact lifecycle policy/u);
  });

  it('rejects a receipt-supplied or unrelated signing key', async () => {
    const value = await fixture();
    const unrelated = generateKeyPairSync('ec', { namedCurve: 'prime256v1' }).privateKey;
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: unrelated
          .export({ format: 'der', type: 'pkcs8' })
          .toString('base64'),
      }),
    ).rejects.toThrow(/does not match/u);
  });
});
