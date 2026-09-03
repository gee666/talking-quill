import { randomBytes } from 'node:crypto';
import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_PHASE_SCHEDULE,
  ACCEPTANCE_REQUEST_SCHEDULE,
  AcceptanceStoppedError,
  HEARTBEAT_READINESS_WINDOW_MS,
  PHYSICAL_OBSERVATION_WINDOW_MS,
  PHYSICAL_TEARDOWN_ALLOWANCE_MS,
  PHYSICAL_TOTAL_BOUND_MS,
  authenticatedUpdateBootstrapArgument,
  createWindowsAcceptanceRunner,
  executeInstalledAcceptance,
  externalTimeout,
  redactEvidence,
  runProductionPhase,
  startTrustedAcceptanceBroker,
  validateAcceptancePhaseStart,
  validateAcceptanceRunSequence,
} from '../../scripts/windows-installed-acceptance.mjs';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';

const RUN_NOT_BEFORE_MS = Date.now() - 1_000;
const RUN_WINDOW = Object.freeze({
  notBeforeMs: RUN_NOT_BEFORE_MS,
  expiresAtMs: RUN_NOT_BEFORE_MS + 80 * 60_000,
  maxTotalRunMs: 80 * 60_000,
});

function frozenSignedRequests() {
  const grouped: Record<string, string | string[]> = {};
  for (const [index, invocation] of ACCEPTANCE_REQUEST_SCHEDULE.entries()) {
    const payload = {
      command: invocation.command,
      buildId: 'aa'.repeat(32),
      invocationId: invocation.invocationId,
      latestStartOffsetMs: invocation.latestStartOffsetMs,
      deadlineOffsetMs: invocation.deadlineOffsetMs,
      runWindow: RUN_WINDOW,
      requestNonce: index.toString(16).padStart(64, '0'),
      issuedAtMs: RUN_NOT_BEFORE_MS + invocation.deadlineOffsetMs - 5 * 60_000,
      expiresAtMs: RUN_NOT_BEFORE_MS + invocation.deadlineOffsetMs,
    };
    const envelope = Buffer.from(
      canonicalAcceptanceJson({ payload, signatureBase64url: 'A'.repeat(86) }),
    ).toString('base64url');
    const current = grouped[invocation.command];
    if (current === undefined) grouped[invocation.command] = envelope;
    else if (Array.isArray(current)) current.push(envelope);
    else grouped[invocation.command] = [current, envelope];
  }
  return Object.freeze(grouped);
}

const plan = {
  architecture: 'x64',
  artifacts: {
    ...Object.fromEntries(
      ['predecessor', 'candidate', 'fresh', 'repair', 'fault'].map((name) => [
        name,
        {
          installer: { path: `${name}.exe`, bytes: 1, sha256: name.padEnd(64, '0') },
          metadataIdentity: { path: `${name}.json`, bytes: 1, sha256: name.padEnd(64, '1') },
          metadata: { packageLayoutDigest: name.padEnd(64, '2') },
        },
      ]),
    ),
    faults: Object.fromEntries(
      ACCEPTANCE_FAULT_PHASES.map((phase) => [
        phase,
        {
          installer: { path: `${phase}.exe`, bytes: 1, sha256: phase.padEnd(64, '0') },
          metadataIdentity: { path: `${phase}.json`, bytes: 1, sha256: phase.padEnd(64, '1') },
          metadata: { packageLayoutDigest: phase.padEnd(64, '2') },
        },
      ]),
    ),
  },
  acceptance: {
    buildId: 'aa'.repeat(32),
    runWindow: RUN_WINDOW,
    signedRequests: frozenSignedRequests(),
  },
  matrix: ACCEPTANCE_MATRIX,
  outputPath: 'evidence.json',
  physicalObservationWindowMs: PHYSICAL_OBSERVATION_WINDOW_MS,
  physicalTeardownAllowanceMs: PHYSICAL_TEARDOWN_ALLOWANCE_MS,
  physicalTotalBoundMs: PHYSICAL_TOTAL_BOUND_MS,
  heartbeatReadinessWindowMs: HEARTBEAT_READINESS_WINDOW_MS,
};

function passingPhase(phase: string): Record<string, unknown> {
  const common = { result: 'passed' };
  return (
    {
      upgrade: { ...common, predecessorAuthenticated: true, candidateInstalled: true },
      'artifact-layout-inspection': {
        ...common,
        exactArtifactHashes: true,
        layoutAuthenticated: true,
      },
      'legacy-cleanup': {
        ...common,
        serviceAbsent: true,
        taskAbsent: true,
        programDataAuthorityAbsent: true,
      },
      'persisted-profile-sentinel-normal-launch': {
        ...common,
        sentinelPreserved: true,
        normalLaunchReady: true,
      },
      'v2-endpoint-peer-checks': { ...common, endpointVersion: 2, peerAuthenticated: true },
      'heartbeat-readiness-120s': {
        ...common,
        stableOwnerIdentity: true,
        leaseExpired: false,
        durationMs: 120_000,
      },
      'neutral-gateway-crash-same-owner-reconnect': {
        ...common,
        neutralAtCrash: true,
        sameOwnerPid: true,
      },
      'lease-expiry': { ...common, expiryObserved: true, captureStayedDisabled: true },
      'electron-crash-relaunch': { ...common, relaunchReady: true, ownerAuthorityUnchanged: true },
      'normal-quit': { ...common, machineQuitObserved: true, singletonReleased: true },
      'login-marker': { ...common, markerPersisted: true, markerConsumedOnce: true },
      'running-silent-repair': {
        ...common,
        authenticatedRepair: true,
        sameCandidate: true,
        sentinelPreserved: true,
      },
      'injected-repair-failure-recovery': {
        ...common,
        failureInjected: true,
        candidateRecoveryCompleted: true,
        mixedAuthorityAbsent: true,
      },
      'uninstall-preserving-data': { ...common, machineFilesAbsent: true, sentinelPreserved: true },
      reinstall: {
        ...common,
        freshInstallerUsed: true,
        sentinelPreserved: true,
        normalLaunchReady: true,
      },
      'diagnostics-disabled-failure': {
        ...common,
        disabledCasePassed: true,
        failureCasePassed: true,
        diagnosticsDidNotControlLifecycle: true,
      },
      'manual-physical-observation': {
        ...common,
        mode: 'passive-physical-observation',
        hardwareEventObserved: true,
        durationMs: 60_000,
        observationWindowMs: PHYSICAL_OBSERVATION_WINDOW_MS,
        teardownAllowanceMs: PHYSICAL_TEARDOWN_ALLOWANCE_MS,
        totalBoundMs: PHYSICAL_TOTAL_BOUND_MS,
      },
      'supplemental-synthetic-observation': {
        ...common,
        authoritative: false,
        label: 'supplemental-non-authoritative',
      },
      residue: { ...common, processesAbsent: true, filesAbsent: true, registrationsAbsent: true },
    }[phase] ?? common
  );
}

function adapters(overrides: Record<string, unknown> = {}) {
  const writes: string[] = [];
  const runner = {
    platform: 'win32',
    architecture: 'x64',
    preflightAcceptance: vi.fn(() =>
      Promise.resolve({
        requestCount: ACCEPTANCE_REQUEST_SCHEDULE.length,
        replayLedgerClear: true,
      }),
    ),
    initialize: vi.fn(() => Promise.resolve({ sentinelSha256: 'aa'.repeat(32) })),
    machineQuit: vi.fn(),
    pollRuntimeExit: vi.fn(),
    probeV1SingletonRelease: vi.fn(() => Promise.resolve({ released: true })),
    processSnapshot: vi.fn(() => Promise.resolve({ ownerReportedMissing: false, liveOwners: [] })),
    collectDiagnostics: vi.fn(() => Promise.resolve({ collected: true })),
    runPhase: vi.fn((phase: string) => Promise.resolve(passingPhase(phase))),
    close: vi.fn(() => Promise.resolve()),
    ...overrides,
  };
  return {
    runner,
    fileSystem: {
      mkdir: vi.fn(),
      writeFile: vi.fn((_path: string, value: string) => {
        writes.push(value);
        return Promise.resolve();
      }),
    },
    writes,
  };
}

describe('installed Windows acceptance executor', () => {
  it('runs acceptance candidate probes before canonical reinstall and external readiness', () => {
    expect(ACCEPTANCE_MATRIX).toEqual([
      'upgrade',
      'artifact-layout-inspection',
      'legacy-cleanup',
      'persisted-profile-sentinel-normal-launch',
      'v2-endpoint-peer-checks',
      'heartbeat-readiness-120s',
      'neutral-gateway-crash-same-owner-reconnect',
      'lease-expiry',
      'electron-crash-relaunch',
      'normal-quit',
      'login-marker',
      'running-silent-repair',
      'injected-repair-failure-recovery',
      'diagnostics-disabled-failure',
      'manual-physical-observation',
      'supplemental-synthetic-observation',
      'uninstall-preserving-data',
      'reinstall',
      'residue',
    ]);
  });

  it('validates the complete frozen invocation sequence with a fake clock', () => {
    const validated = validateAcceptanceRunSequence(plan.acceptance, RUN_NOT_BEFORE_MS);
    expect(validated.requests).toHaveLength(ACCEPTANCE_REQUEST_SCHEDULE.length);
    expect(
      validated.requests.filter(
        (request: { command: string }) => request.command === 'normal-readiness',
      ),
    ).toHaveLength(4);
    for (const phase of ACCEPTANCE_PHASE_SCHEDULE) {
      expect(
        validateAcceptancePhaseStart(
          RUN_WINDOW,
          phase.phase,
          RUN_NOT_BEFORE_MS + phase.latestStartOffsetMs,
        ),
      ).toBe(phase);
    }
    expect(() =>
      validateAcceptancePhaseStart(
        RUN_WINDOW,
        'manual-physical-observation',
        RUN_NOT_BEFORE_MS + 45 * 60_000 + 1,
      ),
    ).toThrow('missed its latest start');
  });

  it('rejects a request that expires before its scheduled deadline before initialize', async () => {
    const signedRequests = { ...plan.acceptance.signedRequests } as Record<
      string,
      string | readonly string[]
    >;
    const encoded = signedRequests['manual-physical-observation'];
    if (typeof encoded !== 'string') throw new Error('Physical request fixture is missing');
    const envelope = JSON.parse(Buffer.from(encoded, 'base64url').toString('utf8')) as {
      payload: Record<string, unknown>;
      signatureBase64url: string;
    };
    envelope.payload.expiresAtMs = RUN_NOT_BEFORE_MS + 59 * 60_000 - 1;
    signedRequests['manual-physical-observation'] = Buffer.from(
      canonicalAcceptanceJson(envelope),
    ).toString('base64url');
    const controlled = adapters();
    await expect(
      executeInstalledAcceptance(
        { ...plan, acceptance: { ...plan.acceptance, signedRequests } },
        controlled,
        { dryRun: false, nowMs: RUN_NOT_BEFORE_MS },
      ),
    ).rejects.toThrow('does not cover its deadline');
    expect(controlled.runner.initialize).not.toHaveBeenCalled();
    expect(controlled.writes).toHaveLength(0);
  });

  it('runs every phase sequentially after machine quit, process retirement, and singleton release', async () => {
    const controlled = adapters();
    const result = await executeInstalledAcceptance(plan, controlled, { dryRun: false });
    expect(result.result).toBe('passed');
    expect(controlled.runner.preflightAcceptance).toHaveBeenCalledOnce();
    expect(controlled.runner.initialize).toHaveBeenCalledOnce();
    expect(controlled.runner.machineQuit).toHaveBeenCalledOnce();
    expect(controlled.runner.pollRuntimeExit).toHaveBeenCalledWith({ timeoutMs: 30_000 });
    expect(controlled.runner.probeV1SingletonRelease).toHaveBeenCalledOnce();
    expect(controlled.runner.close).toHaveBeenCalledOnce();
    expect(controlled.runner.runPhase.mock.calls.map(([phase]) => phase)).toEqual(
      ACCEPTANCE_MATRIX,
    );
    expect(controlled.writes.length).toBeGreaterThan(ACCEPTANCE_MATRIX.length);
  });

  it('stops immediately and collects diagnostics without invoking another phase for owner-missing with a live owner', async () => {
    const controlled = adapters({
      processSnapshot: vi.fn(() =>
        Promise.resolve({ ownerReportedMissing: true, liveOwners: [412] }),
      ),
    });
    await expect(
      executeInstalledAcceptance(plan, controlled, { dryRun: false }),
    ).rejects.toBeInstanceOf(AcceptanceStoppedError);
    expect(controlled.runner.collectDiagnostics).toHaveBeenCalledWith({
      reason: 'owner-missing-live-owner',
    });
    expect(controlled.runner.runPhase).not.toHaveBeenCalled();
  });

  it('converts a phase failure into the terminal live-owner stop when owner-missing appears during the phase', async () => {
    let snapshots = 0;
    const controlled = adapters({
      processSnapshot: vi.fn(() => {
        snapshots += 1;
        return Promise.resolve(
          snapshots < 3
            ? { ownerReportedMissing: false, liveOwners: [] }
            : { ownerReportedMissing: true, liveOwners: [912] },
        );
      }),
      runPhase: vi.fn(() => Promise.reject(new Error('probe disconnected'))),
    });
    await expect(
      executeInstalledAcceptance(plan, controlled, { dryRun: false }),
    ).rejects.toBeInstanceOf(AcceptanceStoppedError);
    expect(controlled.runner.collectDiagnostics).toHaveBeenCalledOnce();
  });

  it('does not grant a physical pass without an observed hardware event', async () => {
    const controlled = adapters({
      runPhase: vi.fn((phase: string) =>
        Promise.resolve(
          phase === 'manual-physical-observation'
            ? { ...passingPhase(phase), hardwareEventObserved: false }
            : passingPhase(phase),
        ),
      ),
    });
    await expect(executeInstalledAcceptance(plan, controlled, { dryRun: false })).rejects.toThrow(
      'hardwareEventObserved',
    );
  });

  it('starts the observation deadline at the explicit fence and keeps teardown separate', async () => {
    const startedAt = Date.now();
    const result = await externalTimeout(60, 20, async (_signal, markStarted) => {
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 15));
      markStarted();
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 60));
      return { result: 'passed', observationDurationMs: 60 };
    });
    expect(result).toMatchObject({
      observationDurationMs: 60,
      observationWindowMs: 60,
      teardownAllowanceMs: 20,
      totalBoundMs: 80,
    });
    expect(Date.now() - startedAt).toBeGreaterThanOrEqual(70);
  });

  it('gives physical observation 60 seconds plus a distinct 20-second teardown allowance', async () => {
    const withExternalTimeout = vi.fn(
      async (
        observationMs: number,
        teardownMs: number,
        operation: (signal: AbortSignal, started: () => void) => Promise<unknown>,
      ) => {
        expect(observationMs).toBe(60_000);
        expect(teardownMs).toBe(20_000);
        const controller = new AbortController();
        return operation(controller.signal, () => undefined);
      },
    );
    const os = {
      utcNow: () => '2026-01-01T00:00:00.000Z',
      withExternalTimeout,
      physicalProbe: vi.fn(() =>
        Promise.resolve({
          result: 'passed',
          mode: 'passive-physical-observation',
          observationDurationMs: 60_000,
          totalElapsedMs: 60_500,
          observationWindowMs: 60_000,
          teardownAllowanceMs: 20_000,
          totalBoundMs: 80_000,
          counterDeltas: {
            physicalCallbacks: 1,
            registeredCandidateCallbacks: 1,
            callbackChannelAccepted: 1,
            adapterDequeued: 1,
            gatewayReceived: 1,
            v10NotificationAccepted: 1,
            electronReceived: 1,
          },
        }),
      ),
    };
    await expect(
      runProductionPhase(
        'manual-physical-observation',
        {
          physicalObservationWindowMs: 60_000,
          physicalTeardownAllowanceMs: 20_000,
        },
        {},
        os,
      ),
    ).resolves.toMatchObject({
      observationWindowMs: 60_000,
      teardownAllowanceMs: 20_000,
      totalBoundMs: 80_000,
    });
    expect(withExternalTimeout).toHaveBeenCalledWith(60_000, 20_000, expect.any(Function));
  });

  it('derives diagnostics privacy and lifecycle evidence from before/after observations', async () => {
    const snapshot = {
      all: [{ pid: 10, parentPid: 1, name: 'owner.exe', pathSha256: '11'.repeat(32) }],
    };
    const identity = {
      releaseBuildDigest: '22'.repeat(32),
      gatewaySha256: '33'.repeat(32),
      ownerSha256: '44'.repeat(32),
    };
    const os = {
      utcNow: () => '2026-01-01T00:00:00.000Z',
      processSnapshot: vi.fn(() => Promise.resolve(snapshot)),
      observeInstalledIdentity: vi.fn(() => Promise.resolve(identity)),
      diagnosticsProbe: vi.fn(() =>
        Promise.resolve({
          disabled: { noDetailedEntries: true },
          failure: { bestEffort: true },
        }),
      ),
      scanEvidenceForForbidden: vi.fn(() =>
        Promise.resolve({ forbiddenMatches: 0, scannedSha256: '55'.repeat(32) }),
      ),
    };
    await expect(
      runProductionPhase('diagnostics-disabled-failure', {}, { installedRoot: 'installed' }, os),
    ).resolves.toMatchObject({
      diagnosticsDidNotControlLifecycle: true,
    });
    expect(os.processSnapshot).toHaveBeenCalledTimes(2);
    expect(os.scanEvidenceForForbidden).toHaveBeenCalledOnce();
  });

  it('dispatches the validated outer release identity to the update bootstrap', () => {
    const releaseIdentity = {
      schemaVersion: 1,
      version: '0.0.69',
      platform: 'win',
      architecture: 'x64',
      ownerMode: 'local-unsigned-enabled',
      packageMode: 'update',
      sourceCommit: '11'.repeat(20),
      sourceTree: '22'.repeat(20),
      releaseBuildDigest: '33'.repeat(32),
      packageLayoutDigest: '33'.repeat(32),
      packageSha256: '44'.repeat(32),
      channel: 'latest-x64',
      transactionBinding: 'source-target-package-sha256-v1',
      roles: [],
      predecessor: {},
      authorization: { scheme: 'p256-sha256-v1', signature: 'signed' },
      acceptancePayload: { schemaVersion: 1 },
    };
    const argument = authenticatedUpdateBootstrapArgument({
      installer: { path: 'candidate.exe', sha256: '44'.repeat(32) },
      metadata: { version: 'forged-inner-value' },
      releaseIdentity,
    });
    const request = JSON.parse(Buffer.from(argument, 'base64').toString('utf8')) as {
      candidate: unknown;
    };
    expect(request.candidate).toMatchObject({
      version: releaseIdentity.version,
      authorization: releaseIdentity.authorization,
    });
    expect(request.candidate).not.toHaveProperty('schemaVersion');
    expect(request.candidate).not.toHaveProperty('acceptancePayload');
    expect(request.candidate).not.toEqual({ version: 'forged-inner-value' });
  });

  it('rehashes each frozen installer immediately before the production upgrade spawns it', async () => {
    const calls: string[] = [];
    const metadata = {
      version: '0.0.69',
      architecture: 'x64',
      releaseBuildDigest: '11'.repeat(32),
      packageLayoutDigest: '11'.repeat(32),
      roles: [
        { role: 'gateway', sha256: '22'.repeat(32) },
        { role: 'owner', sha256: '33'.repeat(32) },
      ],
    };
    const artifact = (name: string) => ({
      installer: { path: `${name}.exe`, bytes: 7, sha256: '44'.repeat(32) },
      metadata,
    });
    const os = {
      utcNow: () => '2026-01-01T00:00:00.000Z',
      hashFile: vi.fn((path: string) => {
        calls.push(`hash:${path}`);
        return Promise.resolve({ path, bytes: 7, sha256: '44'.repeat(32) });
      }),
      spawnTrustedInstaller: vi.fn((request: { installer: { path: string } }) => {
        calls.push(`spawn:${request.installer.path}`);
        return Promise.resolve({ exitCode: 0, evidence: { result: 'passed' } });
      }),
      spawnAuthenticatedUpdate: vi.fn((artifact: { installer: { path: string } }) => {
        calls.push(`authenticated-update:${artifact.installer.path}`);
        return Promise.resolve({ result: 'passed' });
      }),
      machineQuit: vi.fn(() => Promise.resolve()),
      pollRuntimeExit: vi.fn(() => Promise.resolve()),
      observeInstalledIdentity: vi.fn(() =>
        Promise.resolve({
          ...metadata,
          gatewaySha256: '22'.repeat(32),
          ownerSha256: '33'.repeat(32),
        }),
      ),
    };
    const input = {
      artifacts: {
        predecessor: artifact('predecessor'),
        candidate: artifact('candidate'),
      },
    };
    await expect(
      runProductionPhase('upgrade', input, { installedRoot: 'installed' }, os),
    ).resolves.toMatchObject({ result: 'passed' });
    expect(calls).toEqual([
      'hash:predecessor.exe',
      'spawn:predecessor.exe',
      'hash:candidate.exe',
      'authenticated-update:candidate.exe',
    ]);
  });

  it('aborts production execution before spawn when the immediate artifact rehash differs', async () => {
    const os = {
      utcNow: () => '2026-01-01T00:00:00.000Z',
      hashFile: vi.fn(() =>
        Promise.resolve({ path: 'predecessor.exe', bytes: 7, sha256: '00'.repeat(32) }),
      ),
      spawnTrustedInstaller: vi.fn(),
    };
    await expect(
      runProductionPhase(
        'upgrade',
        {
          artifacts: {
            predecessor: {
              installer: { path: 'predecessor.exe', bytes: 7, sha256: '44'.repeat(32) },
            },
          },
        },
        { installedRoot: 'installed' },
        os,
      ),
    ).rejects.toThrow('substituted');
    expect(os.spawnTrustedInstaller).not.toHaveBeenCalled();
  });

  it('passes plan acceptance authorization into default production-adapter construction', () => {
    const adapter = {
      platform: 'win32',
      architecture: 'x64',
      installedRoot: 'installed',
      profileRoot: 'profile',
    };
    const factory = vi.fn(() => adapter);
    const runner = createWindowsAcceptanceRunner(plan, undefined, factory) as {
      platform: string;
    };
    expect(runner.platform).toBe('win32');
    expect(factory).toHaveBeenCalledWith(plan.acceptance);
  });

  it('requires retained-handle process guard evidence for gateway reconnect', async () => {
    const reconnect = {
      neutralAtCrash: true,
      before: { gatewayPid: 41, ownerPid: 52, ownerCreationMarker: '520' },
      after: { gatewayPid: 61, ownerPid: 52, ownerCreationMarker: '520' },
      ownerAliveThroughout: true,
      ownerKernelHandleContinuity: true,
      guardEvidence: {
        result: 'passed',
        exactGatewayHandleTerminated: true,
        gatewayExitObserved: true,
        ownerStayedAlive: true,
        ownerIdentityStable: true,
      },
    };
    const os = {
      utcNow: () => '2026-01-01T00:00:00.000Z',
      gatewayReconnectProbe: vi.fn(() => Promise.resolve(reconnect)),
    };
    await expect(
      runProductionPhase(
        'neutral-gateway-crash-same-owner-reconnect',
        {},
        { installedRoot: 'installed' },
        os,
      ),
    ).resolves.toMatchObject({ result: 'passed', sameOwnerPid: true });
    await expect(
      runProductionPhase(
        'neutral-gateway-crash-same-owner-reconnect',
        {},
        { installedRoot: 'installed' },
        {
          ...os,
          gatewayReconnectProbe: vi.fn(() =>
            Promise.resolve({ ...reconnect, guardEvidence: undefined }),
          ),
        },
      ),
    ).rejects.toThrow('process guard evidence');
  });

  it('starts one frozen broker and never reopens the launcher path for later requests', async () => {
    const launcher = { path: 'acceptance-helper.exe', bytes: 700, sha256: '44'.repeat(32) };
    const stdout = new EventEmitter();
    const child = Object.assign(new EventEmitter(), {
      stdout,
      stdin: {
        write: (encoded: Buffer, callback: (error?: Error) => void) => {
          const request = JSON.parse(encoded.toString('utf8')) as {
            correlation: string;
            operation: string;
            sha256?: string;
          };
          const common = { version: 1, correlation: request.correlation, result: 'passed' };
          const response =
            request.operation === 'hello'
              ? { ...common, brokerSha256: launcher.sha256, brokerBytes: launcher.bytes }
              : request.operation === 'close'
                ? { ...common, closed: true }
                : {
                    ...common,
                    expectedSha256: request.sha256,
                    installerExitCode: 0,
                  };
          queueMicrotask(() => stdout.emit('data', Buffer.from(`${JSON.stringify(response)}\n`)));
          if (request.operation === 'close') setTimeout(() => child.emit('exit', 0), 0);
          callback();
          return true;
        },
        end: vi.fn(),
      },
      kill: vi.fn(),
    });
    let launcherReplaced = false;
    const hashFile = vi.fn(() =>
      Promise.resolve(
        launcherReplaced
          ? { bytes: 1, sha256: '00'.repeat(32) }
          : { bytes: launcher.bytes, sha256: launcher.sha256 },
      ),
    );
    const spawnProcess = vi.fn(() => child);
    const broker = (await startTrustedAcceptanceBroker(launcher, {
      hashFile,
      spawnProcess,
    })) as {
      launchInstaller: (request: Record<string, unknown>) => Promise<unknown>;
      close: () => Promise<void>;
    };
    launcherReplaced = true;
    await expect(
      broker.launchInstaller({
        path: 'candidate.exe',
        sha256: '55'.repeat(32),
        bytes: 9,
        timeoutMs: 1_000,
        acceptedExitCodes: [0],
        arguments: ['/S'],
      }),
    ).resolves.toMatchObject({ result: 'passed', installerExitCode: 0 });
    await broker.close();
    expect(hashFile).toHaveBeenCalledOnce();
    expect(spawnProcess).toHaveBeenCalledOnce();
  });

  it('constructs the production runner over the injected low-level OS adapter', async () => {
    const os = {
      platform: 'win32',
      architecture: 'x64',
      installedRoot: 'installed',
      profileRoot: 'profile',
      mkdir: vi.fn(() => Promise.resolve()),
      writeFile: vi.fn(() => Promise.resolve()),
      startAcceptanceBroker: vi.fn(() => Promise.resolve({ result: 'passed' })),
      closeAcceptanceBroker: vi.fn(() => Promise.resolve()),
      utcNow: () => '2026-01-01T00:00:00.000Z',
      machineQuit: vi.fn(),
      pollRuntimeExit: vi.fn(),
      probeV1SingletonRelease: vi.fn(),
      processSnapshot: vi.fn(),
      collectDiagnostics: vi.fn(),
    };
    const runner = createWindowsAcceptanceRunner(plan, os) as {
      initialize: () => Promise<{ sentinelSha256: string }>;
    };
    const initialized = await runner.initialize();
    expect(initialized.sentinelSha256).toMatch(/^[0-9a-f]{64}$/u);
    expect(os.mkdir).toHaveBeenCalledWith('profile');
    expect(os.writeFile).toHaveBeenCalledOnce();
  });

  it('hashes profile roots, diagnostic locations, pipes, and secrets before persistence', () => {
    const sensitiveValue = randomBytes(32).toString('base64url');
    const redacted = redactEvidence({
      profileRoot: 'C:\\Users\\person\\AppData\\Roaming\\Talking Quill',
      diagnosticArtifactPath: 'C:\\private\\diagnostics\\report.zip',
      responsePipe: '\\\\.\\pipe\\TalkingQuill.Secret',
      privateKey: sensitiveValue,
      result: 'passed',
    });
    const serialized = JSON.stringify(redacted);
    expect(serialized).not.toContain('Users');
    expect(serialized).not.toContain('C:\\\\private');
    expect(serialized).not.toContain('TalkingQuill.Secret');
    expect(serialized).not.toContain(sensitiveValue);
    expect(redacted).toMatchObject({ result: 'passed' });
  });

  it('cryptographically validates but does not reserve or expose bearer values in dry run', async () => {
    const controlled = adapters();
    const result = await executeInstalledAcceptance(plan, controlled);
    expect(result).toMatchObject({ result: 'dry-run' });
    expect(controlled.runner.preflightAcceptance).toHaveBeenCalledWith(
      expect.objectContaining({ reserveNonces: false }),
    );
    expect(JSON.stringify(result)).not.toContain(
      plan.acceptance.signedRequests['endpoint-peer'] as string,
    );
    expect(controlled.runner.initialize).not.toHaveBeenCalled();
    expect(controlled.runner.machineQuit).not.toHaveBeenCalled();
    expect(controlled.runner.runPhase).not.toHaveBeenCalled();
  });
});
