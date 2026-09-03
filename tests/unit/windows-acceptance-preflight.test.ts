import { createHash, generateKeyPairSync, randomBytes, sign, type KeyObject } from 'node:crypto';
import { existsSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import {
  verifyAcceptancePreflight,
  type AcceptancePreflightInput,
} from '../../scripts/windows-acceptance-preflight.mjs';
import {
  acceptanceReplayLedgerPaths,
  reserveAcceptanceRequestNonces,
  type AcceptanceReservationRequest,
} from '../../scripts/windows-acceptance-replay-ledger.mjs';
import {
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_REQUEST_SCHEDULE,
  executeInstalledAcceptance,
} from '../../scripts/windows-installed-acceptance.mjs';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';

const NOW = 1_800_000_000_000;
const BUILD_ID = '11'.repeat(32);
const SOURCE_REVISION = '0123456789ab';
const RUN_WINDOW = Object.freeze({
  notBeforeMs: NOW,
  expiresAtMs: NOW + 80 * 60_000,
  maxTotalRunMs: 80 * 60_000,
});
const reserveAll = (requests: readonly AcceptanceReservationRequest[]) =>
  Promise.resolve({ reservedCount: requests.length });

function publicKey(key: KeyObject): string {
  return Buffer.from(key.export({ format: 'der', type: 'spki' })).toString('base64url');
}

function envelope(payload: unknown, privateKey: KeyObject): string {
  return Buffer.from(
    canonicalAcceptanceJson({
      payload,
      signatureBase64url: sign('sha256', Buffer.from(canonicalAcceptanceJson(payload)), {
        key: privateKey,
        dsaEncoding: 'ieee-p1363',
      }).toString('base64url'),
    }),
  ).toString('base64url');
}

function fixture(
  options: {
    manifestBuildId?: string;
    manifestSourceRevision?: string;
    manifestElectronSha256?: string;
    badRequestSignature?: boolean;
  } = {},
) {
  const manifestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const requestKeys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
  const gatewaySha256 = '22'.repeat(32);
  const ownerSha256 = '33'.repeat(32);
  const ownerManifestSha256 = '44'.repeat(32);
  const releaseBuildDigest = '55'.repeat(32);
  const packageLayoutDigest = '66'.repeat(32);
  const electronSha256 = '77'.repeat(32);
  const appAsarSha256 = '78'.repeat(32);
  const candidate = {
    installer: { path: 'candidate.exe', bytes: 1, sha256: '88'.repeat(32) },
    electron: { path: 'Talking Quill.exe', bytes: 1, sha256: electronSha256 },
    appAsar: { path: 'resources/app.asar', bytes: 1, sha256: appAsarSha256 },
    metadataIdentity: { path: 'candidate.json', bytes: 1, sha256: ownerManifestSha256 },
    metadata: {
      version: '0.0.69',
      releaseBuildDigest,
      packageLayoutDigest,
      roles: [
        { role: 'gateway', sha256: gatewaySha256 },
        { role: 'owner', sha256: ownerSha256 },
      ],
    },
  };
  const grouped: Record<string, string | string[]> = {};
  for (const [index, invocation] of ACCEPTANCE_REQUEST_SCHEDULE.entries()) {
    const payload = {
      version: 1,
      purpose: 'talking-quill/installed-acceptance-run',
      command: invocation.command,
      buildId: BUILD_ID,
      invocationId: invocation.invocationId,
      latestStartOffsetMs: invocation.latestStartOffsetMs,
      deadlineOffsetMs: invocation.deadlineOffsetMs,
      runWindow: RUN_WINDOW,
      requestNonce: index.toString(16).padStart(64, '0'),
      issuedAtMs: NOW + invocation.deadlineOffsetMs - 5 * 60_000,
      expiresAtMs: NOW + invocation.deadlineOffsetMs,
    };
    let encoded = envelope(payload, requestKeys.privateKey);
    if (options.badRequestSignature && index === 0) {
      const decoded = JSON.parse(Buffer.from(encoded, 'base64url').toString('utf8')) as {
        payload: unknown;
        signatureBase64url: string;
      };
      decoded.signatureBase64url = `${decoded.signatureBase64url.slice(0, -1)}${
        decoded.signatureBase64url.endsWith('A') ? 'B' : 'A'
      }`;
      encoded = Buffer.from(canonicalAcceptanceJson(decoded)).toString('base64url');
    }
    const current = grouped[invocation.command];
    if (current === undefined) grouped[invocation.command] = encoded;
    else if (Array.isArray(current)) current.push(encoded);
    else grouped[invocation.command] = [current, encoded];
  }
  const manifestPayload = {
    version: 1,
    purpose: 'talking-quill/installed-acceptance-build',
    sourceRevision: options.manifestSourceRevision ?? SOURCE_REVISION,
    buildId: options.manifestBuildId ?? BUILD_ID,
    architecture: 'x64',
    packageVersion: candidate.metadata.version,
    releaseBuildDigest,
    packageLayoutDigest,
    ownerManifestSha256,
    electronSha256: options.manifestElectronSha256 ?? electronSha256,
    appAsarSha256,
    gatewaySha256,
    ownerSha256,
    requestPublicKeySpkiBase64url: publicKey(requestKeys.publicKey),
    validationPublicKeySpkiBase64url: publicKey(requestKeys.publicKey),
    faultValidationPolicy: {
      schemaVersion: 1,
      phases: [
        'staged',
        'prepared',
        'predecessorMoved',
        'publishing',
        'publishedBeforePersist',
        'published',
        'registered',
        'committed',
        'legacyRetiring',
        'legacyRetired',
      ],
      validatorSha256: 'ab'.repeat(32),
    },
    validFromMs: NOW - 60_000,
    validUntilMs: NOW + 81 * 60_000,
  };
  const buildManifest = envelope(manifestPayload, manifestKeys.privateKey);
  const buildManifestSha256 = createHash('sha256').update(buildManifest).digest('hex');
  const boundCandidate = {
    ...candidate,
    releaseIdentity: {
      acceptancePayload: {
        schemaVersion: 1,
        installerSha256: candidate.installer.sha256,
        electronSha256,
        appAsarSha256,
        buildManifestSha256,
      },
    },
  };
  return {
    schemaVersion: 2,
    architecture: 'x64',
    artifacts: {
      candidate: boundCandidate,
      predecessor: boundCandidate,
      fresh: boundCandidate,
      fault: boundCandidate,
    },
    acceptance: {
      buildId: BUILD_ID,
      sourceRevision: SOURCE_REVISION,
      buildManifest,
      buildManifestIdentity: {
        path: 'resources/windows-installed-acceptance-v1.txt',
        bytes: buildManifest.length,
        sha256: buildManifestSha256,
      },
      manifestPublicKeySpkiBase64url: publicKey(manifestKeys.publicKey),
      runWindow: RUN_WINDOW,
      signedRequests: Object.freeze(grouped),
      acceptanceBroker: { path: 'broker.exe', bytes: 1, sha256: 'ab'.repeat(32) },
      acceptanceBootstrap: { path: 'bootstrap.exe', bytes: 1, sha256: 'ac'.repeat(32) },
      trustedLauncher: { path: 'launcher.exe', bytes: 1, sha256: 'ad'.repeat(32) },
    },
    sourceCommit: '11'.repeat(20),
    sourceTree: '22'.repeat(20),
    candidateInstallerSha256: candidate.installer.sha256,
    bundleSha256: 'ae'.repeat(32),
    matrix: ACCEPTANCE_MATRIX,
    outputPath: 'evidence.json',
    physicalObservationWindowMs: 60_000,
    heartbeatReadinessWindowMs: 120_000,
  };
}

function adapters(reserveReplayNonces: AcceptancePreflightInput['reserveReplayNonces']) {
  const runner = {
    platform: 'win32',
    architecture: 'x64',
    preflightAcceptance: vi.fn((request: Omit<AcceptancePreflightInput, 'reserveReplayNonces'>) =>
      verifyAcceptancePreflight({ ...request, reserveReplayNonces }),
    ),
    initialize: vi.fn(),
    runPhase: vi.fn(),
  };
  const fileSystem = { mkdir: vi.fn(), writeFile: vi.fn() };
  return { runner, fileSystem };
}

describe('Windows acceptance cryptographic preflight', () => {
  it.each([
    [
      'bad request signature',
      fixture({ badRequestSignature: true }),
      reserveAll,
      'signature is invalid',
    ],
    [
      'wrong manifest build',
      fixture({ manifestBuildId: 'aa'.repeat(32) }),
      reserveAll,
      'manifest binding is invalid',
    ],
    [
      'wrong source revision',
      fixture({ manifestSourceRevision: 'abcdefabcdef' }),
      reserveAll,
      'manifest binding is invalid',
    ],
    [
      'wrong Electron identity',
      fixture({ manifestElectronSha256: '99'.repeat(32) }),
      reserveAll,
      'manifest binding is invalid',
    ],
    [
      'reused nonce',
      fixture(),
      () => Promise.reject(new Error('Acceptance request nonce was already consumed')),
      'nonce was already consumed',
    ],
  ])(
    'rejects %s before initialize or phase mutation',
    async (_name, plan, replayCheck, message) => {
      const controlled = adapters(replayCheck);
      await expect(
        executeInstalledAcceptance(plan, controlled, { dryRun: false, nowMs: NOW }),
      ).rejects.toThrow(message);
      expect(controlled.runner.preflightAcceptance).toHaveBeenCalledOnce();
      expect(controlled.runner.initialize).not.toHaveBeenCalled();
      expect(controlled.runner.runPhase).not.toHaveBeenCalled();
      expect(controlled.fileSystem.writeFile).not.toHaveBeenCalled();
    },
  );

  it('verifies every signature without reserving a nonce during dry run', async () => {
    const reserve = vi.fn(reserveAll);
    const controlled = adapters(reserve);
    const result = await executeInstalledAcceptance(fixture(), controlled, {
      nowMs: NOW,
    });
    expect(result).toMatchObject({
      result: 'dry-run',
      validation: {
        requestCount: ACCEPTANCE_REQUEST_SCHEDULE.length,
        reservedNonceCount: 0,
      },
    });
    expect(reserve).not.toHaveBeenCalled();
    expect(controlled.runner.initialize).not.toHaveBeenCalled();
  });

  it('rolls back reservations made before a later nonce conflict', async () => {
    const root = resolve('tmp', `acceptance-rollback-${randomBytes(8).toString('hex')}`);
    const request = (requestNonce: string, invocationId: string): AcceptanceReservationRequest => ({
      invocationId,
      payload: {
        buildId: BUILD_ID,
        requestNonce,
        invocationId,
        runWindow: RUN_WINDOW,
        latestStartOffsetMs: 1,
        deadlineOffsetMs: 2,
        expiresAtMs: NOW + 2,
      },
    });
    const first = request('aa'.repeat(32), 'first');
    const conflicting = request('bb'.repeat(32), 'conflicting');
    try {
      await reserveAcceptanceRequestNonces(root, [conflicting]);
      await expect(reserveAcceptanceRequestNonces(root, [first, conflicting])).rejects.toThrow(
        'already reserved',
      );
      expect(
        existsSync(
          acceptanceReplayLedgerPaths(root, BUILD_ID, first.payload.requestNonce).reserved,
        ),
      ).toBe(false);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });

  it('allows only one concurrent full-sequence reservation before initialize', async () => {
    const root = resolve('tmp', `acceptance-preflight-${randomBytes(8).toString('hex')}`);
    const plan = fixture();
    const reserve = (requests: readonly AcceptanceReservationRequest[]) =>
      reserveAcceptanceRequestNonces(root, requests);
    const first = adapters(reserve);
    const second = adapters(reserve);
    try {
      const outcomes = await Promise.allSettled([
        executeInstalledAcceptance(plan, first, { dryRun: false, nowMs: NOW }),
        executeInstalledAcceptance(plan, second, { dryRun: false, nowMs: NOW }),
      ]);
      const rejectedReservation = outcomes.findIndex(
        (outcome) =>
          outcome.status === 'rejected' &&
          outcome.reason instanceof Error &&
          outcome.reason.message.includes('already reserved'),
      );
      expect(rejectedReservation).toBeGreaterThanOrEqual(0);
      const rejectedRunner = rejectedReservation === 0 ? first.runner : second.runner;
      expect(rejectedRunner.initialize).not.toHaveBeenCalled();
      expect(rejectedRunner.runPhase).not.toHaveBeenCalled();
      expect(
        first.runner.initialize.mock.calls.length + second.runner.initialize.mock.calls.length,
      ).toBe(1);
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
});
