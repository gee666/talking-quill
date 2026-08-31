import { createHash, randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { lstat, mkdir, readFile, readdir, stat, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { basename, resolve } from 'node:path';
import { readNativeArchitectures } from './native-architecture.mjs';
import {
  runPackagedAcceptanceProbe,
  spawnPackagedProcess,
} from './windows-installed-acceptance-probe.mjs';
import { verifyAcceptancePreflight } from './windows-acceptance-preflight.mjs';
import { reserveAcceptanceRequestNonces } from './windows-acceptance-replay-ledger.mjs';
import { ACCEPTANCE_REQUEST_SCHEDULE } from './windows-installed-acceptance-schedule.mjs';

const LEGACY_SERVICE = 'TalkingQuillKeyboardAuthority';
const LEGACY_TASK = 'TalkingQuillKeyboardAuthority';

export function createProductionRunner(plan, os = createWindowsOsAdapter(plan.acceptance)) {
  const state = {
    sentinel: null,
    installedRoot: os.installedRoot,
    candidateIdentity: plan.artifacts.candidate,
  };
  return {
    platform: os.platform,
    architecture: os.architecture,
    preflightAcceptance: (request) => os.preflightAcceptance(request),
    initialize: async () => {
      const broker = await os.startAcceptanceBroker();
      const sentinelPath = resolve(
        os.profileRoot,
        `.installed-acceptance-${randomBytes(16).toString('hex')}.sentinel`,
      );
      const bytes = randomBytes(64);
      await os.mkdir(os.profileRoot);
      await os.writeFile(sentinelPath, bytes);
      state.sentinel = { path: sentinelPath, sha256: sha256(bytes) };
      return {
        broker,
        sentinelPathHash: sha256(Buffer.from(sentinelPath)),
        sentinelSha256: state.sentinel.sha256,
      };
    },
    machineQuit: () => os.machineQuit(),
    pollRuntimeExit: (request) => os.pollRuntimeExit(request.timeoutMs),
    probeV1SingletonRelease: () => os.probeV1SingletonRelease(),
    processSnapshot: () => os.processSnapshot(),
    collectDiagnostics: (request) => os.collectDiagnostics(request),
    runPhase: (phase, input) => runProductionPhase(phase, input, state, os),
    close: () => os.closeAcceptanceBroker(),
  };
}

export async function runProductionPhase(phase, input, state, os) {
  const observations = [];
  const observe = (kind, value) => (
    observations.push({ kind, observedAt: os.utcNow(), value }),
    value
  );
  const installed = () => os.observeInstalledIdentity(state.installedRoot);
  const sentinel = () => verifySentinel(state, os);

  if (phase === 'upgrade') {
    await spawnFrozen(input.artifacts.predecessor, ['/S'], os, observations, [0]);
    const predecessor = observe('installed-predecessor', await installed());
    assertInstalled(predecessor, input.artifacts.predecessor);
    await os.machineQuit();
    await os.pollRuntimeExit(30_000);
    await spawnAuthenticatedUpdate(input.artifacts.candidate, os, observations);
    const candidate = observe(
      'installed-candidate',
      await waitForInstalledArtifact(input.artifacts.candidate, installed, os),
    );
    assertInstalled(candidate, input.artifacts.candidate);
    return pass(observations, { predecessorAuthenticated: true, candidateInstalled: true });
  }
  if (phase === 'artifact-layout-inspection') {
    const identity = observe('installed-layout', await installed());
    assertInstalled(identity, input.artifacts.candidate);
    const tree = observe('installed-tree', await os.inspectTree(state.installedRoot));
    requireValue(tree.reparsePoints.length === 0, 'Installed tree contains a reparse point');
    requireValue(
      tree.nativeArchitectures.length > 0 &&
        tree.nativeArchitectures.every((value) => value.allowed === true),
      'Installed native architecture evidence is missing or mismatched',
    );
    return pass(observations, { exactArtifactHashes: true, layoutAuthenticated: true });
  }
  if (phase === 'legacy-cleanup') {
    const legacy = observe(
      'legacy-state',
      await os.observeLegacyState(LEGACY_SERVICE, LEGACY_TASK),
    );
    requireValue(
      !legacy.service.exists && !legacy.task.exists && legacy.paths.every((value) => !value.exists),
      'Legacy authority remains',
    );
    return pass(observations, {
      serviceAbsent: true,
      taskAbsent: true,
      programDataAuthorityAbsent: true,
    });
  }
  if (phase === 'persisted-profile-sentinel-normal-launch') {
    const sentinelEvidence = observe('profile-sentinel', await sentinel());
    const readiness = observe(
      'normal-readiness',
      await os.appProbe('normal-readiness', { timeoutMs: 45_000, profileRoot: os.profileRoot }),
    );
    requireValue(
      readiness.result === 'passed' &&
        readiness.userDataRootSha256 === sha256(Buffer.from(resolve(os.profileRoot))),
      'Normal persisted-profile readiness failed',
    );
    return pass(observations, {
      sentinelPreserved: sentinelEvidence.preserved,
      normalLaunchReady: true,
    });
  }
  if (phase === 'v2-endpoint-peer-checks') {
    const endpoint = observe('v2-endpoint-peer', await os.endpointPeerProbe());
    const identity = observe('v2-installed-identity', await installed());
    requireValue(
      endpoint.endpointVersion === 2 && endpoint.peerAuthenticated === true,
      'V2 endpoint peer authentication failed',
    );
    requireValue(
      endpoint.gateway.integrityRid === 0x2000 && endpoint.owner.integrityRid === 0x2000,
      'V2 peers are not medium integrity',
    );
    requireValue(
      endpoint.gateway.sessionId === endpoint.owner.sessionId &&
        endpoint.gateway.userSidHash === endpoint.owner.userSidHash &&
        endpoint.releaseBuildDigest === identity.releaseBuildDigest &&
        endpoint.manifestSha256 === identity.ownerManifestSha256,
      'V2 peer security identity differs',
    );
    return pass(observations, { endpointVersion: 2, peerAuthenticated: true });
  }
  if (phase === 'heartbeat-readiness-120s') {
    const heartbeat = observe(
      'heartbeat-samples',
      await os.heartbeatProbe(input.heartbeatReadinessWindowMs),
    );
    requireValue(
      heartbeat.durationMs >= input.heartbeatReadinessWindowMs,
      'Heartbeat duration was short',
    );
    requireValue(
      heartbeat.samples.length > 1 &&
        heartbeat.samples.every(
          (sample) =>
            sample.ready &&
            sample.ownerPid === heartbeat.samples[0].ownerPid &&
            sample.ownerCreationMarker === heartbeat.samples[0].ownerCreationMarker &&
            sample.ownerInstanceId === heartbeat.samples[0].ownerInstanceId,
        ),
      'Heartbeat identity/readiness changed',
    );
    requireValue(
      heartbeat.renewalsAfter > heartbeat.renewalsBefore &&
        heartbeat.expiriesAfter === heartbeat.expiriesBefore,
      'Heartbeat renewal/expiry counters are invalid',
    );
    return pass(observations, {
      stableOwnerIdentity: true,
      leaseExpired: false,
      durationMs: heartbeat.durationMs,
    });
  }
  if (phase === 'neutral-gateway-crash-same-owner-reconnect') {
    const reconnect = observe('gateway-reconnect', await os.gatewayReconnectProbe());
    requireValue(
      reconnect.neutralAtCrash && reconnect.before.gatewayPid !== reconnect.after.gatewayPid,
      'Gateway was not neutrally replaced',
    );
    requireValue(
      reconnect.before.ownerPid === reconnect.after.ownerPid &&
        reconnect.before.ownerCreationMarker === reconnect.after.ownerCreationMarker &&
        reconnect.ownerAliveThroughout &&
        reconnect.ownerKernelHandleContinuity,
      'Owner PID did not remain live',
    );
    requireValue(
      reconnect.guardEvidence?.result === 'passed' &&
        reconnect.guardEvidence.exactGatewayHandleTerminated === true &&
        reconnect.guardEvidence.ownerStayedAlive === true,
      'Trusted gateway process guard evidence is invalid',
    );
    return pass(observations, { neutralAtCrash: true, sameOwnerPid: true });
  }
  if (phase === 'lease-expiry') {
    const expiry = observe('lease-expiry', await os.leaseExpiryProbe());
    requireValue(
      expiry.expiriesAfter === expiry.expiriesBefore + 1 &&
        expiry.captureStayedDisabled &&
        expiry.transactionDelta === 0,
      'Lease expiry evidence is invalid',
    );
    return pass(observations, { expiryObserved: true, captureStayedDisabled: true });
  }
  if (phase === 'electron-crash-relaunch') {
    const crash = observe('electron-crash-relaunch', await os.electronCrashRelaunchProbe());
    requireValue(
      crash.oldElectronExited && crash.relaunch.result === 'passed',
      'Electron crash/relaunch failed',
    );
    requireValue(
      crash.before.ownerSha256 === crash.after.ownerSha256 &&
        crash.before.releaseBuildDigest === crash.after.releaseBuildDigest,
      'Owner authority changed across Electron crash',
    );
    return pass(observations, { relaunchReady: true, ownerAuthorityUnchanged: true });
  }
  if (phase === 'normal-quit') {
    const quit = observe('normal-quit', await os.normalQuitProbe());
    requireValue(
      quit.requestExitCode === 0 && quit.remainingProcesses.length === 0 && quit.singletonReleased,
      'Normal quit did not release runtime',
    );
    return pass(observations, { machineQuitObserved: true, singletonReleased: true });
  }
  if (phase === 'login-marker') {
    const login = observe('login-marker', await os.loginMarkerProbe());
    requireValue(
      login.registrationExact && login.firstLaunchClassified && login.secondLaunchDidNotRestore,
      'Login marker evidence is invalid',
    );
    return pass(observations, { markerPersisted: true, markerConsumedOnce: true });
  }
  if (phase === 'running-silent-repair') {
    await os.ensureApplicationRunning();
    const before = observe('repair-before', await installed());
    const repairProbe = await os.createRepairProbe();
    const beforeSentinel = observe('repair-sentinel-before', await sentinel());
    await spawnFrozen(input.artifacts.candidate, ['/S'], os, observations, [0]);
    const after = observe('repair-after', await installed());
    const afterSentinel = observe('repair-sentinel-after', await sentinel());
    const repairProbeRemoved = !(await os.pathExists(repairProbe));
    assertInstalled(before, input.artifacts.candidate);
    assertInstalled(after, input.artifacts.candidate);
    requireValue(
      beforeSentinel.preserved && afterSentinel.preserved && repairProbeRemoved,
      'Repair did not replace the installed tree while preserving the profile sentinel',
    );
    return pass(observations, {
      authenticatedRepair: true,
      installedTreeReplacementObserved: repairProbeRemoved,
      sameCandidate: true,
      sentinelPreserved: true,
    });
  }
  if (phase === 'injected-precommit-replacement-failure-rollback') {
    const before = observe('fault-before', await installed());
    const machineBefore = observe('fault-machine-before', await os.observeMachineResidue());
    const faults = input.artifacts.faults ?? { published: input.artifacts.fault };
    const crashPhases = [];
    for (const [phaseName, artifact] of Object.entries(faults)) {
      requireValue(artifact.isolatedValidation === true, `${phaseName} fault artifact is not an authenticated isolated-validation build`);
      await spawnFrozen(artifact, ['/S'], os, observations, [197]);
      await spawnFrozen(input.artifacts.candidate, ['/S'], os, observations, [0]);
      crashPhases.push(phaseName);
    }
    const after = observe('fault-after', await installed());
    const machineAfter = observe('fault-machine-after', await os.observeMachineResidue());
    requireValue(
      before.releaseBuildDigest === after.releaseBuildDigest &&
        before.gatewaySha256 === after.gatewaySha256 &&
        before.ownerSha256 === after.ownerSha256,
      'Precommit rollback did not restore predecessor hashes',
    );
    requireValue(
      machineAfter.mixedAuthorityAbsent &&
        machineAfter.transactionsAbsent &&
        machineBefore.legacyServiceRunning === machineAfter.legacyServiceRunning,
      'Rollback machine state is invalid',
    );
    return pass(observations, {
      failureInjected: true,
      crashPhases,
      predecessorRestored: true,
      mixedAuthorityAbsent: true,
    });
  }
  if (phase === 'uninstall-preserving-data') {
    const beforeSentinel = observe('uninstall-sentinel-before', await sentinel());
    await os.normalQuitProbe();
    await os.spawn({
      executable: resolve(state.installedRoot, 'Uninstall Talking Quill.exe'),
      arguments: ['/S'],
      timeoutMs: 180_000,
      acceptedExitCodes: [0],
    });
    const afterSentinel = observe('uninstall-sentinel-after', await sentinel());
    const residue = observe('uninstall-machine-state', await os.observeMachineResidue());
    requireValue(
      beforeSentinel.preserved && afterSentinel.preserved && residue.machineFilesAbsent,
      'Uninstall did not preserve profile or remove machine files',
    );
    return pass(observations, { machineFilesAbsent: true, sentinelPreserved: true });
  }
  if (phase === 'reinstall') {
    await spawnFrozen(input.artifacts.fresh, ['/S'], os, observations, [0]);
    const identity = observe('reinstall-identity', await installed());
    assertInstalled(identity, input.artifacts.fresh);
    const sentinelEvidence = observe('reinstall-sentinel', await sentinel());
    const readiness = observe(
      'reinstall-readiness',
      await os.appProbe('normal-readiness', { timeoutMs: 45_000, profileRoot: os.profileRoot }),
    );
    requireValue(
      sentinelEvidence.preserved && readiness.result === 'passed',
      'Reinstall readiness/sentinel failed',
    );
    return pass(observations, {
      freshInstallerUsed: true,
      sentinelPreserved: true,
      normalLaunchReady: true,
    });
  }
  if (phase === 'diagnostics-disabled-failure') {
    const before = observe('diagnostics-runtime-before', await os.processSnapshot());
    const identityBefore = observe('diagnostics-identity-before', await installed());
    const diagnostics = observe('diagnostics-cases', await os.diagnosticsProbe());
    const after = observe('diagnostics-runtime-after', await os.processSnapshot());
    const identityAfter = observe('diagnostics-identity-after', await installed());
    const privacyScan = observe(
      'diagnostics-evidence-privacy-scan',
      await os.scanEvidenceForForbidden({ before, diagnostics, after }),
    );
    requireValue(
      diagnostics.disabled.noDetailedEntries &&
        diagnostics.failure.bestEffort &&
        JSON.stringify(runtimeIdentity(before)) === JSON.stringify(runtimeIdentity(after)) &&
        identityBefore.releaseBuildDigest === identityAfter.releaseBuildDigest &&
        identityBefore.gatewaySha256 === identityAfter.gatewaySha256 &&
        identityBefore.ownerSha256 === identityAfter.ownerSha256 &&
        privacyScan.forbiddenMatches === 0,
      'Diagnostics disabled/failure privacy or lifecycle evidence failed',
    );
    return pass(observations, {
      disabledCasePassed: true,
      failureCasePassed: true,
      diagnosticsDidNotControlLifecycle: true,
    });
  }
  if (phase === 'manual-physical-observation') {
    const physical = observe(
      'manual-physical',
      await os.withExternalTimeout(
        input.physicalObservationWindowMs,
        input.physicalTeardownAllowanceMs,
        (signal, markObservationStarted) => os.physicalProbe(signal, markObservationStarted),
      ),
    );
    const deltas = physical.counterDeltas ?? {};
    const hardwareOrigin =
      deltas.physicalCallbacks > 0 &&
      deltas.registeredCandidateCallbacks > 0 &&
      deltas.callbackChannelAccepted > 0 &&
      deltas.adapterDequeued > 0 &&
      deltas.gatewayReceived > 0 &&
      deltas.v10NotificationAccepted > 0 &&
      deltas.electronReceived > 0;
    requireValue(
      physical.result === 'passed' &&
        physical.mode === 'passive-physical-observation' &&
        physical.observationDurationMs >= input.physicalObservationWindowMs &&
        physical.totalElapsedMs <=
          input.physicalObservationWindowMs + input.physicalTeardownAllowanceMs &&
        hardwareOrigin,
      'No complete hardware-origin physical traversal was observed',
    );
    return pass(observations, {
      mode: physical.mode,
      hardwareEventObserved: true,
      durationMs: physical.observationDurationMs,
      observationWindowMs: physical.observationWindowMs,
      teardownAllowanceMs: physical.teardownAllowanceMs,
      totalBoundMs: physical.totalBoundMs,
    });
  }
  if (phase === 'supplemental-synthetic-observation') {
    const synthetic = observe('synthetic', await os.syntheticProbe());
    requireValue(
      synthetic.authoritative === false &&
        synthetic.label === 'supplemental-non-authoritative' &&
        synthetic.result === 'passed',
      'Synthetic observation claimed authority or lacked exact traversal evidence',
    );
    return pass(observations, { authoritative: false, label: 'supplemental-non-authoritative' });
  }
  if (phase === 'residue') {
    await os.normalQuitProbe();
    if (await os.pathExists(resolve(state.installedRoot, 'Uninstall Talking Quill.exe'))) {
      await os.spawn({
        executable: resolve(state.installedRoot, 'Uninstall Talking Quill.exe'),
        arguments: ['/S'],
        timeoutMs: 180_000,
        acceptedExitCodes: [0],
      });
    }
    const residue = observe('final-residue', await os.observeMachineResidue());
    const processes = observe('final-processes', await os.processSnapshot());
    requireValue(
      residue.machineFilesAbsent &&
        residue.registrationsAbsent &&
        residue.legacyAbsent &&
        processes.all.length === 0,
      'Final machine residue remains',
    );
    return pass(observations, {
      processesAbsent: true,
      filesAbsent: true,
      registrationsAbsent: true,
    });
  }
  throw new Error(`Unknown installed acceptance phase: ${phase}`);
}

async function waitForInstalledArtifact(artifact, observeInstalled, os) {
  const deadline = Date.now() + 180_000;
  while (true) {
    const identity = await observeInstalled();
    if (identity.releaseBuildDigest === artifact.metadata.releaseBuildDigest) return identity;
    requireValue(Date.now() < deadline, 'Authenticated update did not commit before timeout');
    await (os.sleep?.(250) ?? new Promise((resolvePromise) => setTimeout(resolvePromise, 250)));
  }
}

async function spawnAuthenticatedUpdate(artifact, os, observations) {
  const observed = await os.hashFile(artifact.installer.path);
  observations.push({
    kind: 'pre-spawn-artifact-hash',
    observedAt: os.utcNow(),
    value: { bytes: observed.bytes, sha256: observed.sha256 },
  });
  requireValue(
    observed.sha256 === artifact.installer.sha256 && observed.bytes === artifact.installer.bytes,
    'Frozen installer was substituted before authenticated update launch',
  );
  const launch = await os.spawnAuthenticatedUpdate(artifact);
  observations.push({ kind: 'authenticated-predecessor-update', observedAt: os.utcNow(), value: launch });
}

async function spawnFrozen(artifact, arguments_, os, observations, acceptedExitCodes) {
  const observed = await os.hashFile(artifact.installer.path);
  observations.push({
    kind: 'pre-spawn-artifact-hash',
    observedAt: os.utcNow(),
    value: { bytes: observed.bytes, sha256: observed.sha256 },
  });
  requireValue(
    observed.sha256 === artifact.installer.sha256 && observed.bytes === artifact.installer.bytes,
    'Frozen installer was substituted before spawn',
  );
  const launch = await os.spawnTrustedInstaller({
    installer: artifact.installer,
    arguments: arguments_,
    timeoutMs: 180_000,
    acceptedExitCodes,
  });
  observations.push({
    kind: 'trusted-installer-launch',
    observedAt: os.utcNow(),
    value: launch.evidence,
  });
  return launch;
}
async function verifySentinel(state, os) {
  requireValue(state.sentinel !== null, 'Profile sentinel was not initialized');
  const current = await os.hashFile(state.sentinel.path);
  return { ...current, preserved: current.sha256 === state.sentinel.sha256 };
}
function assertInstalled(observed, artifact) {
  requireValue(
    observed.version === artifact.metadata.version &&
      observed.architecture === artifact.metadata.architecture &&
      observed.releaseBuildDigest === artifact.metadata.releaseBuildDigest &&
      observed.packageLayoutDigest === artifact.metadata.packageLayoutDigest,
    'Installed metadata identity mismatch',
  );
  requireValue(
    observed.gatewaySha256 === role(artifact, 'gateway').sha256 &&
      observed.ownerSha256 === role(artifact, 'owner').sha256,
    'Installed role hash mismatch',
  );
}
function role(artifact, name) {
  return artifact.metadata.roles.find((value) => value.role === name);
}
function runtimeIdentity(snapshot) {
  return [...(snapshot.all ?? [])]
    .map(({ pid, parentPid, name, pathSha256 }) => ({ pid, parentPid, name, pathSha256 }))
    .sort((left, right) => left.pid - right.pid);
}

function pass(observations, fields) {
  return { result: 'passed', observations, ...fields };
}
function requireValue(value, message) {
  if (!value) throw new Error(message);
}
function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

export async function startTrustedAcceptanceBroker(launcher, dependencies = {}) {
  requireValue(launcher !== undefined, 'Frozen trusted acceptance broker is unavailable');
  const hashFile =
    dependencies.hashFile ??
    (async (path) => {
      const bytes = await readFile(path);
      return { bytes: bytes.length, sha256: sha256(bytes) };
    });
  const observed = await hashFile(launcher.path);
  requireValue(
    observed.sha256 === launcher.sha256 && observed.bytes === launcher.bytes,
    'Frozen trusted acceptance broker was substituted before start',
  );
  const spawnProcess =
    dependencies.spawnProcess ??
    ((executable, arguments_) =>
      spawn(executable, arguments_, {
        shell: false,
        windowsHide: true,
        stdio: ['pipe', 'pipe', 'ignore'],
        env: sanitizedChildEnvironment(),
      }));
  const child = spawnProcess(launcher.path, [
    '--windows-installed-acceptance-broker-v1',
    launcher.sha256,
    String(launcher.bytes),
  ]);
  requireValue(child.stdin !== null && child.stdout !== null, 'Trusted broker pipes unavailable');
  const pending = new Map();
  let buffer = Buffer.alloc(0);
  let failed;
  let closed = false;
  let resolveExit;
  const exited = new Promise((resolvePromise) => {
    resolveExit = resolvePromise;
  });
  const fail = (error) => {
    if (failed !== undefined) return;
    failed = error instanceof Error ? error : new Error('Trusted acceptance broker failed');
    for (const value of pending.values()) value.reject(failed);
    pending.clear();
  };
  child.stdout.on('data', (chunk) => {
    if (failed !== undefined) return;
    buffer = Buffer.concat([buffer, chunk]);
    if (buffer.length > 16 * 1024) {
      fail(new Error('Trusted acceptance broker response exceeded its bound'));
      child.kill('SIGKILL');
      return;
    }
    for (;;) {
      const newline = buffer.indexOf(0x0a);
      if (newline < 0) break;
      const frame = buffer.subarray(0, newline);
      buffer = buffer.subarray(newline + 1);
      try {
        if (frame.length === 0 || frame.length > 4 * 1024 || frame.at(-1) === 0x0d) {
          throw new Error('Trusted acceptance broker response framing is invalid');
        }
        const response = JSON.parse(frame.toString('utf8'));
        if (
          response === null ||
          typeof response !== 'object' ||
          response.version !== 1 ||
          !/^[0-9a-f]{32}$/u.test(response.correlation ?? '') ||
          !pending.has(response.correlation)
        ) {
          throw new Error('Trusted acceptance broker response is malformed or uncorrelated');
        }
        const current = pending.get(response.correlation);
        pending.delete(response.correlation);
        clearTimeout(current.timer);
        current.resolve(response);
      } catch (error) {
        fail(error);
        child.kill('SIGKILL');
        return;
      }
    }
  });
  child.once('error', fail);
  child.once('exit', (code) => {
    if (!closed || code !== 0) fail(new Error(`Trusted acceptance broker exited ${String(code)}`));
    resolveExit(code);
  });
  const request = (operation, fields, timeoutMs) => {
    if (failed !== undefined) return Promise.reject(failed);
    if (closed) return Promise.reject(new Error('Trusted acceptance broker is closed'));
    const correlation = randomBytes(16).toString('hex');
    const encoded = Buffer.from(
      `${JSON.stringify({ operation, version: 1, correlation, ...fields })}\n`,
      'utf8',
    );
    if (encoded.length > 16 * 1024) {
      return Promise.reject(new Error('Trusted acceptance broker request exceeded its bound'));
    }
    return new Promise((resolveRequest, rejectRequest) => {
      const timer = setTimeout(() => {
        pending.delete(correlation);
        const error = new Error('Trusted acceptance broker response timed out');
        fail(error);
        child.kill('SIGKILL');
        rejectRequest(error);
      }, timeoutMs);
      pending.set(correlation, { resolve: resolveRequest, reject: rejectRequest, timer });
      child.stdin.write(encoded, (error) => {
        if (error) fail(error);
      });
    });
  };
  const hello = await request('hello', {}, 5_000);
  try {
    requireValue(
      hello.result === 'passed' &&
        hello.brokerSha256 === launcher.sha256 &&
        hello.brokerBytes === launcher.bytes,
      'Trusted acceptance broker did not authenticate its retained image',
    );
  } catch (error) {
    child.kill('SIGKILL');
    throw error;
  }
  return {
    evidence: {
      result: 'passed',
      brokerSha256: hello.brokerSha256,
      brokerBytes: hello.brokerBytes,
    },
    launchInstaller: (fields) => request('launch_installer', fields, fields.timeoutMs + 15_000),
    guardGateway: (fields) => request('guard_gateway', fields, 20_000),
    close: async () => {
      const response = await request('close', {}, 5_000);
      requireValue(response.result === 'passed' && response.closed === true, 'Broker close failed');
      closed = true;
      child.stdin.end();
      let timer;
      const code = await Promise.race([
        exited,
        new Promise((_, reject) => {
          timer = setTimeout(() => {
            child.kill('SIGKILL');
            reject(new Error('Trusted acceptance broker close timed out'));
          }, 5_000);
        }),
      ]).finally(() => clearTimeout(timer));
      if (code !== 0) child.kill('SIGKILL');
      requireValue(code === 0, 'Trusted acceptance broker did not exit cleanly');
    },
  };
}

export function createWindowsOsAdapter(acceptance = {}) {
  const programFiles = process.env.ProgramW6432 ?? process.env.ProgramFiles ?? '';
  const appData = process.env.APPDATA ?? '';
  const installedRoot = resolve(programFiles, 'Talking Quill');
  const profileRoot = resolve(appData, 'Talking Quill');
  const powershell = resolve(
    process.env.SystemRoot ?? 'C:\\Windows',
    'System32/WindowsPowerShell/v1.0/powershell.exe',
  );
  const acceptanceOptions = {
    buildId: acceptance.buildId,
    runWindow: acceptance.runWindow,
    signedRequests: acceptance.signedRequests,
  };
  const commandIndexes = new Map();
  let invocationIndex = 0;
  let broker;
  const probe = (command, options = {}) => {
    const invocation = ACCEPTANCE_REQUEST_SCHEDULE[invocationIndex];
    requireValue(
      /^[0-9a-f]{64}$/u.test(acceptanceOptions.buildId ?? '') && invocation?.command === command,
      'Acceptance request invocation order is invalid',
    );
    const commandIndex = commandIndexes.get(command) ?? 0;
    const source = acceptanceOptions.signedRequests?.[command];
    const signedRequest = Array.isArray(source) ? source[commandIndex] : source;
    requireValue(
      typeof signedRequest === 'string',
      'Acceptance request signing authorization is unavailable',
    );
    commandIndexes.set(command, commandIndex + 1);
    invocationIndex += 1;
    return runPackagedAcceptanceProbe(command, {
      ...acceptanceOptions,
      ...options,
      invocation,
      signedRequest,
      executable: resolve(installedRoot, 'Talking Quill.exe'),
      spawnProcess: spawnPackagedProcess,
    });
  };
  const adapter = {
    platform: process.platform,
    architecture: process.arch,
    installedRoot,
    profileRoot,
    utcNow: () => new Date().toISOString(),
    preflightAcceptance: (request) =>
      verifyAcceptancePreflight({
        ...request,
        reserveReplayNonces: (requests) =>
          reserveAcceptanceRequestNonces(process.env.TEMP ?? process.env.TMP ?? tmpdir(), requests),
      }),
    mkdir: (path) => mkdir(path, { recursive: true }),
    writeFile,
    createRepairProbe: async () => {
      const path = resolve(installedRoot, '.talking-quill-repair-corruption-probe');
      await writeFile(path, randomBytes(64));
      return path;
    },
    pathExists: async (path) =>
      stat(path).then(
        () => true,
        () => false,
      ),
    hashFile: async (path) => {
      const bytes = await readFile(path);
      return { path, bytes: bytes.length, sha256: sha256(bytes) };
    },
    startAcceptanceBroker: async () => {
      requireValue(broker === undefined, 'Trusted acceptance broker was already started');
      broker = await startTrustedAcceptanceBroker(acceptance.trustedLauncher);
      return broker.evidence;
    },
    closeAcceptanceBroker: async () => {
      if (broker === undefined) return;
      const active = broker;
      broker = undefined;
      await active.close();
    },
    guardGatewayProcess: async (request) => {
      requireValue(broker !== undefined, 'Trusted acceptance broker is unavailable');
      for (const process of [request.gateway, request.owner]) {
        requireValue(
          Number.isInteger(process.processId) &&
            process.processId > 0 &&
            /^[1-9][0-9]{0,19}$/u.test(process.creationMarker),
          'Trusted process guard identity is invalid',
        );
      }
      const evidence = await broker.guardGateway({
        gatewayPid: request.gateway.processId,
        gatewayCreationMarker: request.gateway.creationMarker,
        ownerPid: request.owner.processId,
        ownerCreationMarker: request.owner.creationMarker,
      });
      requireValue(
        evidence.result === 'passed' &&
          evidence.gatewayPid === request.gateway.processId &&
          evidence.gatewayCreationMarker === request.gateway.creationMarker &&
          evidence.ownerPid === request.owner.processId &&
          evidence.ownerCreationMarker === request.owner.creationMarker &&
          evidence.exactGatewayHandleTerminated === true &&
          evidence.gatewayExitObserved === true &&
          evidence.ownerStayedAlive === true &&
          evidence.ownerIdentityStable === true,
        'Trusted process guard returned invalid evidence',
      );
      return evidence;
    },
    spawnAuthenticatedUpdate: async (artifact) => {
      const request = Buffer.from(
        JSON.stringify({
          version: 2,
          installerPath: artifact.installer.path,
          sha256: artifact.installer.sha256,
          candidate: artifact.metadata,
        }),
        'utf8',
      ).toString('base64');
      return spawnObserved({
        executable: resolve(installedRoot, 'resources/helper/talking-quill-helper.exe'),
        arguments: [`--windows-update-bootstrap-v2=${request}`],
        timeoutMs: 180_000,
        acceptedExitCodes: [0],
      });
    },
    spawnTrustedInstaller: async (request) => {
      requireValue(broker !== undefined, 'Trusted acceptance broker is unavailable');
      const evidence = await broker.launchInstaller({
        path: request.installer.path,
        sha256: request.installer.sha256,
        bytes: request.installer.bytes,
        timeoutMs: request.timeoutMs,
        acceptedExitCodes: [...request.acceptedExitCodes].sort((left, right) => left - right),
        arguments: request.arguments,
      });
      requireValue(
        evidence.result === 'passed' && evidence.expectedSha256 === request.installer.sha256,
        'Trusted installer broker returned invalid evidence',
      );
      return { exitCode: evidence.installerExitCode, evidence };
    },
    spawn: (request) => spawnObserved(request),
    machineQuit: () =>
      spawnObserved({
        executable: resolve(installedRoot, 'Talking Quill.exe'),
        arguments: ['--talking-quill-request-machine-quit'],
        timeoutMs: 30_000,
        acceptedExitCodes: [0, 2],
      }),
    pollRuntimeExit: (timeoutMs) =>
      spawnObserved({
        executable: powershell,
        arguments: [
          '-NoProfile',
          '-NonInteractive',
          '-Command',
          processWaitScript(installedRoot, timeoutMs),
        ],
        timeoutMs: timeoutMs + 5_000,
        acceptedExitCodes: [0],
      }),
    probeV1SingletonRelease: async () => {
      const value = await spawnObserved({
        executable: resolve(installedRoot, 'resources/helper/talking-quill-keyboard-owner.exe'),
        arguments: ['--probe-owner-singleton-v1'],
        timeoutMs: 15_000,
        acceptedExitCodes: [0],
      });
      return { released: value.exitCode === 0, observation: value };
    },
    processSnapshot: () =>
      jsonPowerShell(powershell, processSnapshotScript(installedRoot, profileRoot)),
    observeInstalledIdentity: (root) => jsonPowerShell(powershell, installedIdentityScript(root)),
    inspectTree: (root) => inspectInstalledTree(root, process.arch),
    observeLegacyState: (service, task) => jsonPowerShell(powershell, legacyScript(service, task)),
    observeMachineResidue: () => jsonPowerShell(powershell, residueScript(installedRoot)),
    collectDiagnostics: (request) =>
      spawnObserved({
        executable: powershell,
        arguments: [
          '-NoProfile',
          '-NonInteractive',
          '-File',
          resolve('scripts/Collect-TalkingQuillDiagnostics.ps1'),
          '-OutputDirectory',
          resolve('tmp/windows-installed-acceptance/diagnostics'),
        ],
        timeoutMs: 180_000,
        acceptedExitCodes: [0, 1],
        environment: { TALKING_QUILL_ACCEPTANCE_REASON: request.reason },
      }),
    appProbe: (name, options) => probe(name, options),
    endpointPeerProbe: async () => (await probe('endpoint-peer', { timeoutMs: 45_000 })).endpoint,
    heartbeatProbe: async (durationMs) =>
      (await probe('heartbeat-120s', { timeoutMs: durationMs + 30_000 })).heartbeatEvidence,
    gatewayReconnectProbe: async () => {
      let guardEvidence;
      const result = await probe('gateway-reconnect-arm', {
        timeoutMs: 75_000,
        onArmed: async (armed) => {
          guardEvidence = await adapter.guardGatewayProcess({
            gateway: armed.endpoint.gateway,
            owner: armed.endpoint.owner,
          });
          return guardEvidence;
        },
      });
      requireValue(guardEvidence?.result === 'passed', 'Trusted process guard evidence is missing');
      const guardBoundToAfterOwner =
        result.after.peerAuthenticated === true &&
        result.after.owner.processId === guardEvidence.ownerPid &&
        result.after.owner.creationMarker === guardEvidence.ownerCreationMarker;
      return {
        neutralAtCrash: result.neutralAtCrash && guardEvidence.gatewayExitObserved === true,
        before: {
          gatewayPid: result.before.gateway.processId,
          ownerPid: result.before.owner.processId,
          ownerCreationMarker: result.before.owner.creationMarker,
        },
        after: {
          gatewayPid: result.after.gateway.processId,
          ownerPid: result.after.owner.processId,
          ownerCreationMarker: result.after.owner.creationMarker,
        },
        guardEvidence,
        ownerAliveThroughout:
          guardEvidence.ownerStayedAlive === true &&
          guardEvidence.ownerIdentityStable === true &&
          guardBoundToAfterOwner,
        ownerKernelHandleContinuity:
          guardEvidence.ownerIdentityStable === true && guardBoundToAfterOwner,
      };
    },
    leaseExpiryProbe: async () => {
      const value = (await probe('lease-expiry-arm', { timeoutMs: 20_000 })).leaseExpiry;
      return {
        expiriesBefore: value.before.leaseExpired,
        expiriesAfter: value.after.leaseExpired,
        captureStayedDisabled:
          value.captureStayedDisabled && value.observedFinalState === 'degraded',
        transactionDelta: value.transactionDelta,
        observation: value,
      };
    },
    electronCrashRelaunchProbe: async () => {
      const before = await jsonPowerShell(powershell, installedIdentityScript(installedRoot));
      const crash = await probe('electron-crash-arm', {
        timeoutMs: 30_000,
        onArmed: async (_armed, child) => terminateSingleProcess(child.pid),
      });
      const relaunch = await probe('normal-readiness', { timeoutMs: 45_000 });
      const after = await jsonPowerShell(powershell, installedIdentityScript(installedRoot));
      return { oldElectronExited: crash.oldElectronExited, relaunch, before, after };
    },
    normalQuitProbe: async () => {
      const request = await adapter.machineQuit();
      await adapter.pollRuntimeExit(30_000);
      const remaining = await adapter.processSnapshot();
      const singleton = await adapter.probeV1SingletonRelease();
      return {
        requestExitCode: request.exitCode,
        remainingProcesses: remaining.all,
        singletonReleased: singleton.released,
      };
    },
    loginMarkerProbe: async () => {
      const registration = await jsonPowerShell(powershell, loginRegistrationScript(installedRoot));
      const launch = await probe('login-marker', {
        timeoutMs: 45_000,
        onArmed: async () => {
          const second = spawnPackagedProcess(
            resolve(installedRoot, 'Talking Quill.exe'),
            ['--talking-quill-login-start'],
            15_000,
          );
          return second.exited;
        },
      });
      return {
        registrationExact: registration.exact,
        firstLaunchClassified:
          launch.windowsLoginStart === true && launch.mainWindowVisible === false,
        secondLaunchDidNotRestore:
          launch.secondLaunchIgnored === true && launch.mainWindowVisible === false,
      };
    },
    scanEvidenceForForbidden: async (value) => {
      const sensitiveValues = [
        profileRoot,
        installedRoot,
        resolve('tmp/windows-installed-acceptance/diagnostics'),
      ].filter((candidate) => candidate.length > 0);
      let redacted = JSON.stringify(value);
      for (const candidate of sensitiveValues) {
        redacted = redacted.replaceAll(candidate, `[sha256:${sha256(Buffer.from(candidate))}]`);
      }
      const forbiddenPatterns = [
        /[A-Za-z]:[\\/](?:Users|ProgramData|Program Files)[\\/]/iu,
        /\\\\\.\\pipe\\/u,
        /S-1-(?:\d+-)+\d+/u,
        /-----BEGIN [A-Z ]*PRIVATE KEY-----/u,
        /TALKING_QUILL_.*(?:PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u,
      ];
      const forbiddenMatches = forbiddenPatterns.filter((pattern) => pattern.test(redacted)).length;
      return {
        forbiddenMatches,
        redactedEvidenceSha256: sha256(Buffer.from(redacted)),
        scannedBytes: Buffer.byteLength(redacted),
      };
    },
    diagnosticsProbe: async () => {
      const result = await probe('diagnostics-disabled-failure', { timeoutMs: 45_000 });
      return {
        disabled: { noDetailedEntries: result.diagnostics.enabled === false },
        failure: { bestEffort: result.diagnostics.injectedFailureContained === true },
        observation: result.diagnostics,
      };
    },
    physicalProbe: async (signal, markObservationStarted) => {
      const result = await probe('manual-physical-observation', {
        timeoutMs: 125_000,
        signal,
        onObservationStarted: markObservationStarted,
      });
      return result;
    },
    syntheticProbe: () =>
      probe('supplemental-synthetic-observation', {
        timeoutMs: 45_000,
        onArmed: async () => {
          const sender = acceptance.syntheticSender;
          requireValue(sender !== undefined, 'Frozen synthetic sender is unavailable');
          const observed = await adapter.hashFile(sender.path);
          requireValue(
            observed.sha256 === sender.sha256 && observed.bytes === sender.bytes,
            'Frozen synthetic sender was substituted before spawn',
          );
          await adapter.spawn({
            executable: sender.path,
            arguments: acceptance.syntheticSenderArguments ?? [],
            timeoutMs: 15_000,
            acceptedExitCodes: [0],
          });
        },
      }),
    ensureApplicationRunning: () => probe('normal-readiness', { timeoutMs: 45_000 }),
    withExternalTimeout: (observationMs, teardownMs, operation) =>
      externalTimeout(observationMs, teardownMs, operation),
  };
  return adapter;
}

function spawnObserved(request) {
  return new Promise((resolveSpawn, reject) => {
    const startedAt = new Date().toISOString();
    const child = spawn(request.executable, request.arguments, {
      shell: false,
      windowsHide: true,
      stdio: 'pipe',
      env: sanitizedChildEnvironment(request.environment),
    });
    let output = '';
    const append = (chunk) => {
      output = `${output}${chunk.toString()}`.slice(-64 * 1024);
    };
    child.stdout?.on('data', append);
    child.stderr?.on('data', append);
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      if (child.pid === undefined) return;
      const killer = spawn('taskkill.exe', ['/PID', String(child.pid), '/T', '/F'], {
        shell: false,
        windowsHide: true,
        stdio: 'ignore',
      });
      killer.once('error', () => child.kill('SIGKILL'));
    }, request.timeoutMs);
    child.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once('exit', (exitCode) => {
      clearTimeout(timer);
      if (timedOut) {
        reject(new Error(`${basename(request.executable)} timed out and was retired`));
        return;
      }
      const observation = {
        executable: request.executable,
        arguments: request.arguments,
        pid: child.pid,
        startedAt,
        endedAt: new Date().toISOString(),
        exitCode,
        output,
      };
      if (request.acceptedExitCodes.includes(exitCode ?? -1)) resolveSpawn(observation);
      else reject(new Error(`${basename(request.executable)} exited ${String(exitCode)}`));
    });
  });
}
function sanitizedChildEnvironment(overrides = {}) {
  return Object.fromEntries(
    Object.entries({ ...process.env, ...overrides }).filter(
      ([name]) => !/^TALKING_QUILL_.*(?:PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u.test(name),
    ),
  );
}

async function jsonPowerShell(executable, script) {
  const value = await spawnObserved({
    executable,
    arguments: ['-NoProfile', '-NonInteractive', '-Command', script],
    timeoutMs: 30_000,
    acceptedExitCodes: [0],
  });
  return JSON.parse(value.output.trim() || '{}');
}
async function terminateSingleProcess(processId) {
  requireValue(Number.isInteger(processId) && processId > 0, 'Invalid process ID for retirement');
  return spawnObserved({
    executable: 'taskkill.exe',
    arguments: ['/PID', String(processId), '/F'],
    timeoutMs: 15_000,
    acceptedExitCodes: [0],
  });
}

export async function externalTimeout(observationMs, teardownMs, operation) {
  const controller = new AbortController();
  let observationTimer;
  let hardTimer;
  let startupTimer;
  let markStarted;
  const started = new Promise((resolveStarted) => {
    markStarted = () => resolveStarted(Date.now());
  });
  const running = operation(controller.signal, markStarted);
  try {
    const observationStartedAt = await Promise.race([
      started,
      running.then(() => {
        throw new Error('Physical probe exited before the observation window started');
      }),
      new Promise((_, reject) => {
        startupTimer = setTimeout(
          () => reject(new Error('Physical probe did not reach its observation start fence')),
          45_000,
        );
      }),
    ]);
    clearTimeout(startupTimer);
    const gracefulTeardownMs = Math.min(5_000, teardownMs);
    observationTimer = setTimeout(() => controller.abort(), observationMs + gracefulTeardownMs);
    const result = await Promise.race([
      running,
      new Promise((_, reject) => {
        hardTimer = setTimeout(
          () => reject(new Error('Physical probe teardown exceeded its separate allowance')),
          observationMs + teardownMs,
        );
      }),
    ]);
    return {
      ...result,
      observationDurationMs: result.observationDurationMs,
      totalElapsedMs: Date.now() - observationStartedAt,
      observationWindowMs: observationMs,
      teardownAllowanceMs: teardownMs,
      totalBoundMs: observationMs + teardownMs,
    };
  } finally {
    controller.abort();
    clearTimeout(startupTimer);
    clearTimeout(observationTimer);
    clearTimeout(hardTimer);
  }
}
function processWaitScript(root, timeoutMs) {
  return `$d=[DateTime]::UtcNow.AddMilliseconds(${timeoutMs});do{$p=@(Get-CimInstance Win32_Process|?{$_.ExecutablePath -and $_.ExecutablePath.StartsWith('${ps(root)}\\',[StringComparison]::OrdinalIgnoreCase)});if($p.Count-eq0){exit 0};Start-Sleep -Milliseconds 100}while([DateTime]::UtcNow-lt$d);exit 75`;
}
function processSnapshotScript(root, profileRoot) {
  return `$r='${ps(root)}\\';$p=@(Get-CimInstance Win32_Process|?{$_.ExecutablePath -and $_.ExecutablePath.StartsWith($r,[StringComparison]::OrdinalIgnoreCase)}|%{[pscustomobject]@{pid=$_.ProcessId;parentPid=$_.ParentProcessId;name=$_.Name;pathSha256=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes([string]$_.ExecutablePath))).ToLowerInvariant()}});$log='${ps(profileRoot)}\\logs\\diagnostic.jsonl';$last=$null;if(Test-Path -LiteralPath $log){Get-Content -LiteralPath $log -Tail 256|%{try{$v=$_|ConvertFrom-Json;if($v.event-eq'helper.readiness.changed'){$last=$v}}catch{}}};[pscustomobject]@{all=$p;liveOwners=@($p|?{$_.name -ieq 'talking-quill-keyboard-owner.exe'}|%{$_.pid});ownerReportedMissing=$null-ne$last-and$last.metadata.reason-eq'owner-missing';readinessObservedAt=if($null-eq$last){$null}else{$last.timestamp}}|ConvertTo-Json -Depth 5 -Compress`;
}
function installedIdentityScript(root) {
  return `$r='${ps(root)}';$mp=Join-Path $r 'resources\\keyboard-owner-release-v1.json';$m=Get-Content -Raw -LiteralPath $mp|ConvertFrom-Json;$g=$m.roles|? role -eq gateway;$o=$m.roles|? role -eq owner;[pscustomobject]@{version=$m.version;architecture=$m.architecture;releaseBuildDigest=$m.releaseBuildDigest;packageLayoutDigest=$m.packageLayoutDigest;ownerManifestSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath $mp).Hash.ToLowerInvariant();gatewaySha256=(Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $r $g.path)).Hash.ToLowerInvariant();ownerSha256=(Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $r $o.path)).Hash.ToLowerInvariant()}|ConvertTo-Json -Compress`;
}
async function inspectInstalledTree(root, architecture) {
  const reparsePoints = [];
  const nativeArchitectures = [];
  const pending = [root];
  while (pending.length > 0) {
    const directory = pending.pop();
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const path = resolve(directory, entry.name);
      const metadata = await lstat(path);
      if (metadata.isSymbolicLink() || (metadata.mode & 0o170000) === 0o120000) {
        reparsePoints.push(path);
        continue;
      }
      if (entry.isDirectory()) {
        pending.push(path);
        continue;
      }
      if (/\.(?:exe|dll|node)$/iu.test(entry.name)) {
        const observed = await readNativeArchitectures(path);
        const allowed =
          entry.name.toLowerCase() === 'elevate.exe'
            ? observed.includes('ia32')
            : observed.length === 1 && observed[0] === architecture;
        nativeArchitectures.push({ path, observed, allowed });
      }
    }
  }
  return { reparsePoints, nativeArchitectures };
}
function legacyScript(service, task) {
  return `$s=Get-Service -Name '${ps(service)}' -ErrorAction SilentlyContinue;$t=Get-ScheduledTask -TaskName '${ps(task)}' -ErrorAction SilentlyContinue;$pd=[Environment]::GetFolderPath('CommonApplicationData');$paths=@((Join-Path $pd 'Talking Quill\\KeyboardAuthority'),(Join-Path $pd 'Talking Quill\\.KeyboardAuthority.retirement-quarantine'));[pscustomobject]@{service=@{exists=$null-ne$s};task=@{exists=$null-ne$t};paths=@($paths|%{@{path=$_;exists=Test-Path -LiteralPath $_}})}|ConvertTo-Json -Depth 5 -Compress`;
}
function loginRegistrationScript(root) {
  return `$expected='"${ps(root)}\\Talking Quill.exe" --talking-quill-login-start';$entries=@(Get-ItemProperty 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Run' -ErrorAction SilentlyContinue);$actual=$entries.'Talking Quill';$hash=$null;if($null-ne$actual){$hash=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes([string]$actual))).ToLowerInvariant()};[pscustomobject]@{exact=$actual-eq$expected;valueHash=$hash}|ConvertTo-Json -Compress`;
}

function residueScript(root) {
  return `$pf=Split-Path -Parent '${ps(root)}';$pd=[Environment]::GetFolderPath('CommonApplicationData');$paths=@('${ps(root)}',(Join-Path $pf '.Talking Quill.stage1-backup'),(Join-Path $pf '.Talking Quill.stage1-ambiguous-replacement'),(Join-Path $pf '.Talking Quill.stage1-transaction.json'),(Join-Path $pd 'Talking Quill\\KeyboardAuthority'),(Join-Path $pd 'Talking Quill\\.KeyboardAuthority.retirement-quarantine'));$present=@($paths|?{Test-Path -LiteralPath $_});$service=Get-Service -Name '${LEGACY_SERVICE}' -ErrorAction SilentlyContinue;$task=Get-ScheduledTask -TaskName '${LEGACY_TASK}' -ErrorAction SilentlyContinue;$uninstall=@(Get-ItemProperty 'HKLM:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*','HKLM:\\Software\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*','HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\*' -ErrorAction SilentlyContinue|?{$_.DisplayName -eq 'Talking Quill'});$transactions=@($paths|?{$_ -like '*.stage1-*' -and (Test-Path -LiteralPath $_)});[pscustomobject]@{machineFilesAbsent=$present.Count-eq0;registrationsAbsent=$uninstall.Count-eq0;legacyAbsent=$null-eq$service-and$null-eq$task-and-not(Test-Path (Join-Path $pd 'Talking Quill\\KeyboardAuthority'));mixedAuthorityAbsent=$null-eq$service-and$null-eq$task;transactionsAbsent=$transactions.Count-eq0;legacyServiceRunning=$null-ne$service-and$service.Status-eq'Running'}|ConvertTo-Json -Compress`;
}
function ps(value) {
  return String(value).replaceAll("'", "''");
}
