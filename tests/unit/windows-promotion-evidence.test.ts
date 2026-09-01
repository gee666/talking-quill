import { createHash, generateKeyPairSync, sign } from 'node:crypto';
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
const terminalFaultPhases = [
  'pre-CreateService',
  'post-service-pre-record',
  'post-record-pre-start',
  'failure-action-restart',
  'service-stopped-pre-DeleteService',
  'post-delete-pre-image-removal',
  'pre-maintenance-deletion-ownership',
  'post-maintenance-deletion-ownership',
  'post-final-launcher-ownership',
  'post-uninstall-unregister',
  'post-journal-removal',
  'post-root-tombstone-rename',
  'post-tombstone-content-removal',
  'post-tombstone-record-removal',
  'post-tombstone-marker-removal',
  'post-tombstone-removal',
  'post-maintenance-posix-delete',
  'post-final-deletion-ownership',
  'post-final-launcher-posix-delete',
  'pre-machine-relaunch-owner-clear',
  'post-machine-relaunch-owner-clear',
  'post-owner-clear-posix-cleanup',
];
const digest = (bytes: Buffer) => createHash('sha256').update(bytes).digest();
const le32 = (value: number) => {
  const bytes = Buffer.alloc(4);
  bytes.writeUInt32LE(value);
  return bytes;
};

function authenticationReceipt(action: 'install' | 'repair' | 'uninstall') {
  const { privateKey, publicKey } = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const point = (key: typeof publicKey) => {
    const jwk = key.export({ format: 'jwk' });
    if (jwk.x === undefined || jwk.y === undefined)
      throw new Error('Generated P-256 key is invalid');
    return Buffer.concat([
      Buffer.from([4]),
      Buffer.from(jwk.x, 'base64url'),
      Buffer.from(jwk.y, 'base64url'),
    ]);
  };
  const controllerPublic = point(publicKey);
  const workerPublic = Buffer.from(controllerPublic);
  const request = Buffer.alloc(6);
  request[0] = { install: 1, repair: 2, uninstall: 3 }[action];
  request[1] = 1;
  const nonce = Buffer.alloc(32, 2);
  const peer = Buffer.alloc(32, 3);
  const workerProof = Buffer.alloc(32, 4);
  const controllerProof = Buffer.alloc(32, 5);
  const transcript = digest(
    Buffer.concat([
      Buffer.from('TalkingQuill/setup-authenticated-transcript/v1'),
      nonce,
      controllerPublic,
      workerPublic,
      peer,
      request,
      workerProof,
      controllerProof,
    ]),
  );
  const challenge = Buffer.alloc(32, 6);
  const packageHash = Buffer.from(sha('a'), 'hex');
  const signed = Buffer.concat([
    Buffer.from('TalkingQuill/setup-evidence-signature/v1'),
    challenge,
    le32(100),
    le32(101),
    packageHash,
    peer,
    transcript,
    nonce,
    controllerPublic,
    workerPublic,
    request,
    workerProof,
    controllerProof,
  ]);
  return {
    schemaVersion: 2,
    controllerPid: 100,
    workerPid: 101,
    packageSha256: packageHash.toString('hex'),
    peerBinding: peer.toString('hex'),
    transcriptSha256: transcript.toString('hex'),
    observerChallenge: challenge.toString('hex'),
    nonce: nonce.toString('hex'),
    controllerPublicKey: controllerPublic.toString('hex'),
    workerPublicKey: workerPublic.toString('hex'),
    request: request.toString('hex'),
    workerProof: workerProof.toString('hex'),
    controllerProof: controllerProof.toString('hex'),
    evidencePublicKey: controllerPublic.toString('hex'),
    evidenceSignature: sign('sha256', signed, {
      key: privateKey,
      dsaEncoding: 'ieee-p1363',
    }).toString('hex'),
  };
}

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
    nativeAuthenticationReceipt: authenticationReceipt(
      operation === 'fresh' ? 'install' : 'repair',
    ),
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
    faults: [
      'committed',
      'legacyRetired',
      'legacyRetiring',
      'predecessorMoved',
      'prepared',
      'published',
      'publishedBeforePersist',
      'publishing',
      'registered',
      'staged',
    ].map((phase) => ({
      fault: `Talking-Quill-test-repair-${phase}.exe`,
      crashExitCode: 197,
      recoveryExitCode: 78,
      inspectedBeforeRepair: true,
      productionRecoveryInterrupted: true,
      recoveredReleaseBuildDigest: sha('d'),
      appPathExact: true,
      quietUninstallExact: true,
      legacyServiceAbsent: true,
      legacyTaskAbsent: true,
    })),
    terminalCleanup: {
      acceptanceSetupSha256: 'ef'.repeat(32),
      inspected: true,
      scenarios: terminalFaultPhases.map((phase) => ({
        phase,
        acceptanceSetupSha256: 'ef'.repeat(32),
        installedUninstallerSha256: 'ef'.repeat(32),
        isolated: true,
        recovered: true,
      })),
      residue: [] as string[],
      registry: [] as string[],
    },
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
  const publicKeyPath = join(root, 'promotion-key.sec1');
  await writeFile(publicKeyPath, sec1.toString('hex'));
  const canonical = (value: unknown): string => {
    if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
    if (value !== null && typeof value === 'object')
      return `{${Object.entries(value)
        .sort(([left], [right]) => left.localeCompare(right))
        .map(([key, item]) => `${JSON.stringify(key)}:${canonical(item)}`)
        .join(',')}}`;
    return JSON.stringify(value);
  };
  for (const architecture of ['arm64', 'x64']) {
    const payload = {
      schemaVersion: 1,
      architecture,
      candidateSha256: 'ef'.repeat(32),
      sourceRevision: source('b'),
      workflowRunId: architecture === 'x64' ? '456' : '789',
      workflowRunAttempt: 1,
      checkpointSha256: sha('7'),
      machineIdentity: `machine-${architecture}`,
      preBootIdentity: 'boot-before',
      postBootIdentity: 'boot-after',
      generationBefore: '1'.repeat(32),
      generationAfter: '2'.repeat(32),
      terminalGeneration: '3'.repeat(32),
      pendingDeleteSources: [
        `\\??\\C:\\ProgramData\\.Talking Quill Terminal Cleanup-${'3'.repeat(32)}.exe`,
      ],
      windowsConsumedPendingDeletes: true,
    };
    const signature = sign(
      'sha256',
      Buffer.concat([
        Buffer.from('TalkingQuill/windows-real-reboot-acceptance/v1\0'),
        Buffer.from(canonical(payload)),
      ]),
      { key: privateKey, dsaEncoding: 'ieee-p1363' },
    );
    await writeFile(
      join(directory, `windows-reboot-acceptance-${architecture}.json`),
      JSON.stringify({
        schemaVersion: 1,
        payload,
        publicKeySha256: createHash('sha256')
          .update(publicKey.export({ format: 'der', type: 'spki' }))
          .digest('hex'),
        signature: signature.toString('base64url'),
      }),
    );
  }
  const updater = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const updaterJwk = updater.publicKey.export({ format: 'jwk' });
  if (updaterJwk.x === undefined || updaterJwk.y === undefined)
    throw new Error('Generated updater P-256 key is invalid');
  const updaterSec1 = Buffer.concat([
    Buffer.from([4]),
    Buffer.from(updaterJwk.x, 'base64url'),
    Buffer.from(updaterJwk.y, 'base64url'),
  ]);
  const updatePublicKeyPath = join(root, 'update-key.sec1');
  await writeFile(updatePublicKeyPath, updaterSec1.toString('hex'));
  return {
    directory,
    publicKeyPath,
    updatePublicKeyPath,
    rebootRunIds: { x64: '456', arm64: '789' },
    output: join(directory, 'windows-promotion-lifecycle-evidence-v1.json'),
    privateKey: privateKey.export({ format: 'der', type: 'pkcs8' }).toString('base64'),
    updatePrivateKeyPkcs8Base64: updater.privateKey
      .export({ format: 'der', type: 'pkcs8' })
      .toString('base64'),
  };
}

describe('Windows promotion lifecycle evidence', () => {
  it('binds exact lifecycle claims to the pinned protected P-256 release key', async () => {
    const value = await fixture();
    const created = await createWindowsPromotionEvidence({
      ...value,
      repository: 'owner/repository',
      workflowRunId: '123',
      privateKeyPkcs8Base64: value.privateKey,
    });
    const records = created.payload.records as {
      claims: {
        kind: string;
        action: string;
        terminalCleanup?: {
          acceptanceSetupSha256: string;
          inspected: boolean;
          scenarios: unknown[];
          residue: unknown[];
          registry: unknown[];
        };
      };
    }[];
    expect(
      records
        .filter(({ claims }) => claims.kind === 'fault')
        .every(({ claims }) => claims.action === 'fault'),
    ).toBe(true);
    expect(records.find(({ claims }) => claims.kind === 'fault')?.claims.terminalCleanup).toEqual(
      expect.objectContaining({
        acceptanceSetupSha256: 'ef'.repeat(32),
        inspected: true,
        residue: [],
        registry: [],
      }),
    );
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
        rebootRunIds: value.rebootRunIds,
      }),
    ).rejects.toThrow(/did not pass exact lifecycle policy/u);
  });

  it('rejects malformed terminal cleanup and fault topology before signing', async () => {
    const value = await fixture();
    const path = join(value.directory, 'windows-installer-fault-recovery-x64.json');
    const invalidCleanup = fault('x64');
    invalidCleanup.terminalCleanup.residue.push('leftover.exe');
    await writeFile(path, JSON.stringify(invalidCleanup));
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.privateKey,
      }),
    ).rejects.toThrow(/terminal cleanup evidence is invalid/u);

    const invalidFault = fault('x64');
    delete (invalidFault.faults[0] as Partial<(typeof invalidFault.faults)[number]>).appPathExact;
    await writeFile(path, JSON.stringify(invalidFault));
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.privateKey,
      }),
    ).rejects.toThrow(/unexpected schema/u);

    const duplicateFault = fault('x64');
    const [firstFault, secondFault] = duplicateFault.faults;
    if (firstFault === undefined || secondFault === undefined)
      throw new Error('Missing fault fixture');
    firstFault.fault = secondFault.fault;
    await writeFile(path, JSON.stringify(duplicateFault));
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.privateKey,
      }),
    ).rejects.toThrow(/fault phase inventory is invalid/u);
  });

  it('binds terminal cleanup and package identity into the signed architecture generation', async () => {
    const value = await fixture();
    const path = join(value.directory, 'windows-installer-fault-recovery-x64.json');
    await writeFile(path, JSON.stringify({ ...fault('x64'), candidateSha256: sha('9') }));
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.privateKey,
      }),
    ).rejects.toThrow(/generation binding is invalid/u);
  });

  it('rejects a signed receipt whose authenticated action differs from its lifecycle claim', async () => {
    const value = await fixture();
    const path = join(value.directory, 'windows-installer-success-x64.json');
    await writeFile(
      path,
      JSON.stringify({
        ...success('x64', 'repair'),
        nativeAuthenticationReceipt: authenticationReceipt('install'),
      }),
    );
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.privateKey,
      }),
    ).rejects.toThrow(/action does not match/u);
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

  it('rejects reuse of the updater signing key for promotion evidence', async () => {
    const value = await fixture();
    await expect(
      createWindowsPromotionEvidence({
        ...value,
        repository: 'owner/repository',
        workflowRunId: '123',
        privateKeyPkcs8Base64: value.updatePrivateKeyPkcs8Base64,
      }),
    ).rejects.toThrow(/promotion key/u);
  });
});
