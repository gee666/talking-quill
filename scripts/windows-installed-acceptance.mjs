import { createHash } from 'node:crypto';
import { lstat, mkdir, readFile, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validatePackageReleaseMetadata } from './release-package-metadata.mjs';
import { parseTqpkg2 } from './tqpkg2.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import {
  ACCEPTANCE_PHASE_SCHEDULE,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_RUN_MS,
} from './windows-installed-acceptance-schedule.mjs';
import {
  createProductionRunner,
  createWindowsOsAdapter,
  externalTimeout,
  runProductionPhase,
  startTrustedAcceptanceBroker,
} from './windows-installed-acceptance-runner.mjs';

export {
  createWindowsOsAdapter,
  externalTimeout,
  runProductionPhase,
  startTrustedAcceptanceBroker,
};

export const PHYSICAL_OBSERVATION_WINDOW_MS = 60_000;
export const PHYSICAL_TEARDOWN_ALLOWANCE_MS = 20_000;
export const PHYSICAL_TOTAL_BOUND_MS =
  PHYSICAL_OBSERVATION_WINDOW_MS + PHYSICAL_TEARDOWN_ALLOWANCE_MS;
export const HEARTBEAT_READINESS_WINDOW_MS = 120_000;
export { ACCEPTANCE_PHASE_SCHEDULE, ACCEPTANCE_REQUEST_SCHEDULE, MAX_ACCEPTANCE_RUN_MS };
export const ACCEPTANCE_MATRIX = Object.freeze([
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
  'uninstall-preserving-data',
  'reinstall',
  'diagnostics-disabled-failure',
  'manual-physical-observation',
  'supplemental-synthetic-observation',
  'residue',
]);

export class AcceptanceStoppedError extends Error {
  constructor(message, evidence) {
    super(message);
    this.name = 'AcceptanceStoppedError';
    this.evidence = evidence;
  }
}

export async function createInstalledAcceptancePlan(input, fileSystem = nodeFileSystem()) {
  if (!['x64', 'arm64'].includes(input.architecture)) {
    throw new Error('Acceptance requires an exact native x64 or arm64 architecture');
  }
  const artifacts = {};
  for (const name of ['predecessor', 'candidate', 'fresh', 'repair', 'fault']) {
    artifacts[name] = await freezeArtifact(
      name,
      input.artifacts?.[name],
      input.architecture,
      fileSystem,
    );
  }
  if (input.artifacts?.faults !== undefined) {
    artifacts.faults = {};
    for (const phase of [
      'staged',
      'prepared',
      'predecessorMoved',
      'published',
      'registered',
      'committed',
      'legacyRetiring',
      'legacyRetired',
    ]) {
      const faultInput = input.artifacts.faults[phase];
      if (faultInput === undefined)
        throw new Error(`Acceptance fault artifact is missing for ${phase}`);
      artifacts.faults[phase] = await freezeArtifact(
        'fault',
        faultInput,
        input.architecture,
        fileSystem,
      );
    }
  }
  const predecessor = artifacts.predecessor.metadata;
  const candidate = artifacts.candidate.metadata;
  const fresh = artifacts.fresh.metadata;
  const repair = artifacts.repair.metadata;
  if (
    candidate.packageMode !== 'update' ||
    candidate.predecessor?.version !== predecessor.version ||
    candidate.predecessor.releaseBuildDigest !== predecessor.releaseBuildDigest ||
    candidate.predecessor.gatewaySha256 !== role(predecessor, 'gateway').sha256 ||
    candidate.predecessor.ownerSha256 !== role(predecessor, 'owner').sha256
  )
    throw new Error('Candidate does not authenticate the exact frozen predecessor');
  if (fresh.packageMode !== 'fresh' || fresh.freshInstall !== true || fresh.predecessor !== null) {
    throw new Error('Reinstall artifact is not a fresh installer');
  }
  if (
    repair.packageMode !== 'repair' ||
    repair.predecessor !== null ||
    repair.version !== candidate.version ||
    repair.sourceCommit !== candidate.sourceCommit ||
    repair.sourceTree !== candidate.sourceTree ||
    artifacts.repair.packageManifest?.target.releaseBuildDigest !== candidate.releaseBuildDigest ||
    artifacts.repair.packageManifest?.target.gatewaySha256 !== role(candidate, 'gateway').sha256 ||
    artifacts.repair.packageManifest?.target.ownerSha256 !== role(candidate, 'owner').sha256 ||
    role(repair, 'gateway').sha256 !== role(candidate, 'gateway').sha256 ||
    role(repair, 'owner').sha256 !== role(candidate, 'owner').sha256
  ) {
    throw new Error('Repair artifact is not bound to the exact candidate identity');
  }
  if (
    artifacts.fault.metadata.version !== candidate.version ||
    artifacts.fault.metadata.packageLayoutDigest !== candidate.packageLayoutDigest ||
    artifacts.fault.metadata.releaseBuildDigest !== candidate.releaseBuildDigest
  )
    throw new Error('Fault-injection artifact is not bound to the exact candidate layout');
  const embeddedBuildManifestPath = resolve(
    artifacts.candidate.unpackedRoot,
    'resources/windows-installed-acceptance-v1.txt',
  );
  if (resolve(input.acceptance?.buildManifestPath ?? '') !== embeddedBuildManifestPath) {
    throw new Error('Acceptance build manifest is not the frozen candidate embedded manifest');
  }
  const buildManifestFile = await regularIdentity(
    embeddedBuildManifestPath,
    input.acceptance?.buildManifestSha256,
    fileSystem,
  );
  const buildManifest = (await fileSystem.readFile(buildManifestFile.path)).toString('utf8').trim();
  if (!/^[A-Za-z0-9_-]+$/u.test(buildManifest)) {
    throw new Error('Frozen acceptance build manifest is invalid');
  }
  const candidateAcceptanceBinding = artifacts.candidate.releaseIdentity?.acceptancePayload;
  if (
    candidateAcceptanceBinding?.schemaVersion !== 1 ||
    candidateAcceptanceBinding?.electronSha256 !== artifacts.candidate.electron.sha256 ||
    candidateAcceptanceBinding?.appAsarSha256 !== artifacts.candidate.appAsar.sha256 ||
    candidateAcceptanceBinding?.buildManifestSha256 !== buildManifestFile.sha256 ||
    candidateAcceptanceBinding?.installerSha256 !== artifacts.candidate.installer.sha256
  ) {
    throw new Error('Candidate release identity does not bind the acceptance payload');
  }
  const requestsFile = await regularIdentity(
    input.acceptance?.signedRequestsPath,
    input.acceptance?.signedRequestsSha256,
    fileSystem,
  );
  const signedRequests = JSON.parse(
    (await fileSystem.readFile(requestsFile.path)).toString('utf8'),
  );
  const runWindow = freezeRunWindow(input.acceptance?.runWindow);
  const acceptance = Object.freeze({
    buildId: requireHex(input.acceptance?.buildId, 'Acceptance build ID'),
    sourceRevision: requireSourceRevision(input.acceptance?.sourceRevision),
    buildManifest,
    buildManifestIdentity: buildManifestFile,
    manifestPublicKeySpkiBase64url: requireBase64Url(
      input.acceptance?.manifestPublicKeySpkiBase64url,
      'Acceptance manifest public key',
    ),
    runWindow,
    signedRequests: freezeSignedRequests(signedRequests),
    signedRequestsIdentity: requestsFile,
    syntheticSender: await regularIdentity(
      input.acceptance?.syntheticSenderPath,
      input.acceptance?.syntheticSenderSha256,
      fileSystem,
    ),
    trustedLauncher: await regularIdentity(
      input.acceptance?.trustedLauncherPath,
      input.acceptance?.trustedLauncherSha256,
      fileSystem,
    ),
    syntheticSenderArguments: Object.freeze(input.acceptance?.syntheticSenderArguments ?? []),
  });
  validateAcceptanceRunSequence(acceptance);
  return Object.freeze({
    schemaVersion: 2,
    architecture: input.architecture,
    artifacts: Object.freeze(artifacts),
    acceptance,
    matrix: ACCEPTANCE_MATRIX,
    outputPath: resolve(input.outputPath ?? 'tmp/windows-installed-acceptance/evidence.json'),
    physicalObservationWindowMs: PHYSICAL_OBSERVATION_WINDOW_MS,
    physicalTeardownAllowanceMs: PHYSICAL_TEARDOWN_ALLOWANCE_MS,
    physicalTotalBoundMs: PHYSICAL_TOTAL_BOUND_MS,
    heartbeatReadinessWindowMs: HEARTBEAT_READINESS_WINDOW_MS,
  });
}

export async function executeInstalledAcceptance(plan, adapters, options = {}) {
  const { runner, fileSystem } = adapters;
  if (options.dryRun !== false) return Object.freeze({ result: 'dry-run', plan });
  if (runner.platform !== 'win32' || runner.architecture !== plan.architecture) {
    throw new Error('Installed acceptance requires an exact native Windows host');
  }
  const now =
    typeof options.nowMs === 'function' ? options.nowMs : () => options.nowMs ?? Date.now();
  const preflightNowMs = now();
  const sequence = validateAcceptanceRunSequence(plan.acceptance, preflightNowMs);
  if (typeof runner.preflightAcceptance !== 'function') {
    throw new Error('Acceptance runner preflight is unavailable');
  }
  const preflight = await runner.preflightAcceptance({ plan, sequence, nowMs: preflightNowMs });
  const evidence = {
    schemaVersion: 2,
    result: 'running',
    architecture: plan.architecture,
    artifacts: artifactEvidence(plan.artifacts),
    preflight: redactEvidence(preflight),
    phases: [],
  };
  await persist(fileSystem, plan.outputPath, evidence);
  const context = { plan, evidence };
  try {
    const initialization = await runner.initialize();
    evidence.initialization = initialization;
    await persist(fileSystem, plan.outputPath, evidence);
    await guardOwnerMissing(context, adapters, 'initial');
    for (const phase of ACCEPTANCE_MATRIX) {
      validateAcceptancePhaseStart(plan.acceptance.runWindow, phase, now());
      await guardOwnerMissing(context, adapters, `before-${phase}`);
      let result;
      try {
        result = await runner.runPhase(phase, phaseInput(phase, plan));
      } catch (error) {
        await guardOwnerMissing(context, adapters, `failed-${phase}`);
        throw error;
      }
      validatePhase(phase, result, plan);
      evidence.phases.push({ phase, result: redactEvidence(result) });
      await persist(fileSystem, plan.outputPath, evidence);
      await guardOwnerMissing(context, adapters, `after-${phase}`);
      if (phase === 'upgrade') {
        await runner.machineQuit();
        await runner.pollRuntimeExit({ timeoutMs: 30_000 });
        const singleton = await runner.probeV1SingletonRelease();
        requireFields(singleton, { released: true }, 'V1 singleton-release probe');
      }
    }
    await runner.close?.();
    evidence.result = 'passed';
    await persist(fileSystem, plan.outputPath, evidence);
    return Object.freeze(redactEvidence(evidence));
  } catch (error) {
    await runner.close?.().catch(() => undefined);
    evidence.result = 'failed';
    evidence.failure = error instanceof Error ? error.message : 'unknown failure';
    await persist(fileSystem, plan.outputPath, evidence).catch(() => undefined);
    throw error;
  }
}

async function guardOwnerMissing(context, adapters, boundary) {
  const snapshot = await adapters.runner.processSnapshot();
  if (snapshot.ownerReportedMissing === true && snapshot.liveOwners?.length > 0) {
    const diagnostics = await adapters.runner.collectDiagnostics({
      reason: 'owner-missing-live-owner',
    });
    const stop = redactEvidence({
      boundary,
      snapshot,
      diagnostics,
      furtherAcceptanceStopped: true,
    });
    context.evidence.phases.push({ phase: 'owner-missing-live-owner-stop', result: stop });
    await persist(adapters.fileSystem, context.plan.outputPath, context.evidence);
    throw new AcceptanceStoppedError(
      'Owner was reported missing while an owner process remained live',
      stop,
    );
  }
}

function phaseInput(phase, plan) {
  return Object.freeze({
    phase,
    artifacts: plan.artifacts,
    physicalObservationWindowMs: plan.physicalObservationWindowMs,
    physicalTeardownAllowanceMs: plan.physicalTeardownAllowanceMs,
    physicalTotalBoundMs: plan.physicalTotalBoundMs,
    heartbeatReadinessWindowMs: plan.heartbeatReadinessWindowMs,
    syntheticAuthoritative: false,
    acceptance: plan.acceptance,
  });
}

function validatePhase(phase, value, plan) {
  if (value?.result !== 'passed') throw new Error(`${phase} did not pass`);
  const expected = {
    upgrade: { predecessorAuthenticated: true, candidateInstalled: true },
    'artifact-layout-inspection': { exactArtifactHashes: true, layoutAuthenticated: true },
    'legacy-cleanup': { serviceAbsent: true, taskAbsent: true, programDataAuthorityAbsent: true },
    'persisted-profile-sentinel-normal-launch': {
      sentinelPreserved: true,
      normalLaunchReady: true,
    },
    'v2-endpoint-peer-checks': { endpointVersion: 2, peerAuthenticated: true },
    'heartbeat-readiness-120s': { stableOwnerIdentity: true, leaseExpired: false },
    'neutral-gateway-crash-same-owner-reconnect': { neutralAtCrash: true, sameOwnerPid: true },
    'lease-expiry': { expiryObserved: true, captureStayedDisabled: true },
    'electron-crash-relaunch': { relaunchReady: true, ownerAuthorityUnchanged: true },
    'normal-quit': { machineQuitObserved: true, singletonReleased: true },
    'login-marker': { markerPersisted: true, markerConsumedOnce: true },
    'running-silent-repair': {
      authenticatedRepair: true,
      sameCandidate: true,
      sentinelPreserved: true,
    },
    'injected-repair-failure-recovery': {
      failureInjected: true,
      candidateRecoveryCompleted: true,
      mixedAuthorityAbsent: true,
    },
    'uninstall-preserving-data': { machineFilesAbsent: true, sentinelPreserved: true },
    reinstall: { freshInstallerUsed: true, sentinelPreserved: true, normalLaunchReady: true },
    'diagnostics-disabled-failure': {
      disabledCasePassed: true,
      failureCasePassed: true,
      diagnosticsDidNotControlLifecycle: true,
    },
    residue: { processesAbsent: true, filesAbsent: true, registrationsAbsent: true },
  }[phase];
  if (expected) requireFields(value, expected, phase);
  if (phase === 'heartbeat-readiness-120s' && value.durationMs < plan.heartbeatReadinessWindowMs) {
    throw new Error('Heartbeat readiness observation was shorter than 120 seconds');
  }
  if (phase === 'manual-physical-observation') {
    requireFields(
      value,
      { mode: 'passive-physical-observation', hardwareEventObserved: true },
      phase,
    );
    requireFields(
      value,
      {
        observationWindowMs: plan.physicalObservationWindowMs,
        teardownAllowanceMs: plan.physicalTeardownAllowanceMs,
        totalBoundMs: plan.physicalTotalBoundMs,
      },
      phase,
    );
    if (
      value.durationMs < plan.physicalObservationWindowMs ||
      value.durationMs > plan.physicalObservationWindowMs + 1_000
    ) {
      throw new Error('Physical observation did not span its exact 60-second window');
    }
  }
  if (phase === 'supplemental-synthetic-observation') {
    requireFields(value, { authoritative: false, label: 'supplemental-non-authoritative' }, phase);
  }
}

async function freezeArtifact(name, input, architecture, fileSystem) {
  if (!input) throw new Error(`Missing frozen ${name} artifact evidence`);
  const installer = await regularIdentity(input.installerPath, input.installerSha256, fileSystem);
  const metadataIdentity = await regularIdentity(
    input.metadataPath,
    input.metadataSha256,
    fileSystem,
  );
  const metadata = validatePackageReleaseMetadata(
    JSON.parse((await fileSystem.readFile(metadataIdentity.path)).toString('utf8')),
  );
  if (metadata.platform !== 'win' || metadata.architecture !== architecture) {
    throw new Error(`${name} metadata architecture is not exact`);
  }
  const unpackedRoot = resolve(input.unpackedRoot);
  let electron = null;
  let appAsar = null;
  if (name === 'candidate') {
    const expectedElectronPath = resolve(unpackedRoot, 'Talking Quill.exe');
    if (resolve(input.electronPath ?? '') !== expectedElectronPath) {
      throw new Error('Candidate Electron identity is not inside the frozen unpacked root');
    }
    electron = await regularIdentity(input.electronPath, input.electronSha256, fileSystem);
    const expectedAppAsarPath = resolve(unpackedRoot, 'resources/app.asar');
    if (resolve(input.appAsarPath ?? '') !== expectedAppAsarPath) {
      throw new Error('Candidate app.asar identity is not inside the frozen unpacked root');
    }
    appAsar = await regularIdentity(input.appAsarPath, input.appAsarSha256, fileSystem);
  }
  for (const current of metadata.roles) {
    const identity = await regularIdentity(
      resolve(unpackedRoot, current.path),
      current.sha256,
      fileSystem,
    );
    if (identity.sha256 !== current.sha256) throw new Error(`${name} unpacked role hash mismatch`);
  }
  let releaseIdentity = null;
  if (input.releaseIdentityPath) {
    const releaseIdentityFile = await regularIdentity(
      input.releaseIdentityPath,
      input.releaseIdentitySha256,
      fileSystem,
    );
    releaseIdentity = JSON.parse(
      (await fileSystem.readFile(releaseIdentityFile.path)).toString('utf8'),
    );
    if (
      releaseIdentity.packageSha256 !== installer.sha256 ||
      releaseIdentity.packageLayoutDigest !== metadata.packageLayoutDigest ||
      JSON.stringify(releaseIdentity.roles) !== JSON.stringify(metadata.roles)
    )
      throw new Error(`${name} release identity does not bind installer and unpacked layout`);
  } else if (name === 'candidate' || name === 'predecessor') {
    throw new Error(`${name} release identity is required`);
  }
  const nativePackage = /\.exe$/iu.test(installer.path)
    ? parseTqpkg2(await fileSystem.readFile(installer.path), metadata.architecture, {
        allowAcceptanceFaults: name === 'fault',
      })
    : null;
  if (nativePackage !== null && nativePackage.manifest.packageMode !== metadata.packageMode) {
    throw new Error(`${name} TQPKG2 mode does not match release metadata`);
  }
  let isolatedValidation = false;
  if (name === 'fault') {
    const validationFile = await regularIdentity(
      input.validationEvidencePath,
      input.validationEvidenceSha256,
      fileSystem,
    );
    const validation = JSON.parse(
      (await fileSystem.readFile(validationFile.path)).toString('utf8'),
    );
    if (
      validation.isolatedInstallValidation !== true ||
      validation.failurePoint !== nativePackage?.manifest.faultPhase ||
      ![
        'staged',
        'prepared',
        'predecessorMoved',
        'published',
        'registered',
        'committed',
        'legacyRetiring',
        'legacyRetired',
      ].includes(validation.failurePoint) ||
      validation.candidatePackageLayoutDigest !== metadata.packageLayoutDigest
    ) {
      throw new Error('Fault artifact validation evidence is invalid');
    }
    isolatedValidation = true;
  }
  return Object.freeze({
    name,
    installer,
    metadataIdentity,
    unpackedRoot,
    metadata,
    releaseIdentity,
    electron,
    appAsar,
    isolatedValidation,
    packageManifest: nativePackage?.manifest ?? null,
  });
}

export function validateAcceptanceRunSequence(acceptance, nowMs) {
  const buildId = requireHex(acceptance?.buildId, 'Acceptance build ID');
  const runWindow = freezeRunWindow(acceptance?.runWindow);
  if (nowMs !== undefined) validateRunClock(runWindow, nowMs);
  const expectedCommands = new Set(ACCEPTANCE_REQUEST_SCHEDULE.map((entry) => entry.command));
  const actualCommands = Object.keys(acceptance?.signedRequests ?? {});
  if (
    actualCommands.length !== expectedCommands.size ||
    actualCommands.some((command) => !expectedCommands.has(command))
  ) {
    throw new Error('Frozen signed acceptance request command set is invalid');
  }
  const commandIndexes = new Map();
  const nonces = new Set();
  const requests = ACCEPTANCE_REQUEST_SCHEDULE.map((invocation) => {
    const commandIndex = commandIndexes.get(invocation.command) ?? 0;
    commandIndexes.set(invocation.command, commandIndex + 1);
    const source = acceptance.signedRequests[invocation.command];
    const encoded = Array.isArray(source)
      ? source[commandIndex]
      : commandIndex === 0
        ? source
        : null;
    const expectedCount = ACCEPTANCE_REQUEST_SCHEDULE.filter(
      (entry) => entry.command === invocation.command,
    ).length;
    if (
      typeof encoded !== 'string' ||
      !/^[A-Za-z0-9_-]+$/u.test(encoded) ||
      (expectedCount > 1 && (!Array.isArray(source) || source.length !== expectedCount)) ||
      (expectedCount === 1 && Array.isArray(source))
    ) {
      throw new Error(`Frozen signed acceptance request is missing: ${invocation.invocationId}`);
    }
    const envelope = decodeFrozenRequest(encoded);
    const payload = envelope.payload;
    if (
      payload.command !== invocation.command ||
      payload.buildId !== buildId ||
      payload.invocationId !== invocation.invocationId ||
      payload.latestStartOffsetMs !== invocation.latestStartOffsetMs ||
      payload.deadlineOffsetMs !== invocation.deadlineOffsetMs ||
      canonicalAcceptanceJson(payload.runWindow) !== canonicalAcceptanceJson(runWindow)
    ) {
      throw new Error(
        `Frozen signed acceptance request binding is invalid: ${invocation.invocationId}`,
      );
    }
    const invocationDeadline = runWindow.notBeforeMs + invocation.deadlineOffsetMs;
    if (
      !Number.isSafeInteger(payload.issuedAtMs) ||
      !Number.isSafeInteger(payload.expiresAtMs) ||
      payload.issuedAtMs >= payload.expiresAtMs ||
      payload.expiresAtMs - payload.issuedAtMs > MAX_ACCEPTANCE_RUN_MS ||
      (nowMs !== undefined && nowMs > payload.expiresAtMs) ||
      payload.expiresAtMs < invocationDeadline ||
      payload.expiresAtMs > runWindow.expiresAtMs
    ) {
      throw new Error(
        `Frozen signed acceptance request does not cover its deadline: ${invocation.invocationId}`,
      );
    }
    if (!/^[0-9a-f]{64}$/u.test(payload.requestNonce ?? '') || nonces.has(payload.requestNonce)) {
      throw new Error('Frozen signed acceptance request nonce is invalid or repeated');
    }
    nonces.add(payload.requestNonce);
    return Object.freeze({ ...invocation, encoded, payload: Object.freeze(payload) });
  });
  return Object.freeze({ buildId, runWindow, requests: Object.freeze(requests) });
}

export function validateAcceptancePhaseStart(runWindowInput, phaseName, nowMs) {
  const runWindow = freezeRunWindow(runWindowInput);
  validateRunClock(runWindow, nowMs);
  const phase = ACCEPTANCE_PHASE_SCHEDULE.find((entry) => entry.phase === phaseName);
  if (phase === undefined) throw new Error(`Acceptance phase schedule is missing: ${phaseName}`);
  if (nowMs > runWindow.notBeforeMs + phase.latestStartOffsetMs) {
    throw new Error(`Acceptance phase missed its latest start: ${phaseName}`);
  }
  return phase;
}

function validateRunClock(runWindow, nowMs) {
  if (!Number.isSafeInteger(nowMs)) throw new Error('Acceptance clock is invalid');
  const effectiveExpiry = Math.min(
    runWindow.expiresAtMs,
    runWindow.notBeforeMs + runWindow.maxTotalRunMs,
  );
  if (nowMs < runWindow.notBeforeMs) throw new Error('Acceptance run window has not opened');
  if (nowMs > effectiveExpiry) throw new Error('Acceptance run window has expired');
}

function freezeRunWindow(value) {
  if (
    value === null ||
    typeof value !== 'object' ||
    !Number.isSafeInteger(value.notBeforeMs) ||
    !Number.isSafeInteger(value.expiresAtMs) ||
    !Number.isSafeInteger(value.maxTotalRunMs) ||
    value.notBeforeMs < 0 ||
    value.expiresAtMs <= value.notBeforeMs ||
    value.maxTotalRunMs <= 0 ||
    value.maxTotalRunMs > MAX_ACCEPTANCE_RUN_MS ||
    value.expiresAtMs - value.notBeforeMs < value.maxTotalRunMs ||
    ACCEPTANCE_PHASE_SCHEDULE.some((entry) => entry.deadlineOffsetMs > value.maxTotalRunMs)
  ) {
    throw new Error('Frozen acceptance run window is invalid');
  }
  return Object.freeze({
    notBeforeMs: value.notBeforeMs,
    expiresAtMs: value.expiresAtMs,
    maxTotalRunMs: value.maxTotalRunMs,
  });
}

function freezeSignedRequests(value) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Frozen signed acceptance requests are invalid');
  }
  return Object.freeze(
    Object.fromEntries(
      Object.entries(value).map(([command, requests]) => [
        command,
        Array.isArray(requests) ? Object.freeze([...requests]) : requests,
      ]),
    ),
  );
}

function decodeFrozenRequest(encoded) {
  let bytes;
  let envelope;
  try {
    bytes = Buffer.from(encoded, 'base64url');
    if (bytes.length === 0 || bytes.length > 16 * 1024 || bytes.toString('base64url') !== encoded) {
      throw new Error('encoding');
    }
    envelope = JSON.parse(bytes.toString('utf8'));
  } catch {
    throw new Error('Frozen signed acceptance request is invalid');
  }
  if (
    Buffer.from(canonicalAcceptanceJson(envelope)).toString('base64url') !== encoded ||
    envelope === null ||
    typeof envelope !== 'object' ||
    envelope.payload === null ||
    typeof envelope.payload !== 'object' ||
    !/^[A-Za-z0-9_-]{86}$/u.test(envelope.signatureBase64url ?? '')
  ) {
    throw new Error('Frozen signed acceptance request is invalid');
  }
  return envelope;
}

function requireSourceRevision(value) {
  if (typeof value !== 'string' || !/^[0-9a-f]{12}$/u.test(value)) {
    throw new Error('Acceptance source revision is invalid');
  }
  return value;
}

function requireBase64Url(value, label) {
  if (typeof value !== 'string' || !/^[A-Za-z0-9_-]+$/u.test(value) || value.length > 256) {
    throw new Error(`${label} is invalid`);
  }
  return value;
}

function requireHex(value, label) {
  if (!/^[0-9a-f]{64}$/u.test(value ?? '')) throw new Error(`${label} is invalid`);
  return value;
}

async function regularIdentity(path, expectedSha256, fileSystem) {
  if (!/^[0-9a-f]{64}$/u.test(expectedSha256 ?? ''))
    throw new Error(`Invalid expected SHA-256: ${basename(path ?? '')}`);
  const absolute = resolve(path);
  const [stat, bytes] = await Promise.all([
    fileSystem.lstat(absolute),
    fileSystem.readFile(absolute),
  ]);
  if (!stat.isFile() || stat.isSymbolicLink())
    throw new Error(`Frozen input is not a regular file: ${basename(absolute)}`);
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  if (sha256 !== expectedSha256)
    throw new Error(`Frozen input SHA-256 mismatch: ${basename(absolute)}`);
  return Object.freeze({ path: absolute, bytes: stat.size, sha256 });
}
function role(metadata, name) {
  const value = metadata.roles.find((entry) => entry.role === name);
  if (!value) throw new Error(`Missing ${name} role`);
  return value;
}
function requireFields(value, expected, name) {
  for (const [key, expectedValue] of Object.entries(expected)) {
    if (value?.[key] !== expectedValue) throw new Error(`${name} evidence is missing ${key}`);
  }
}
function artifactEvidence(artifacts) {
  return Object.fromEntries(
    Object.entries(artifacts).map(([name, value]) => [
      name,
      {
        installer: identityEvidence(value.installer),
        metadata: identityEvidence(value.metadataIdentity),
        ...(value.electron === null || value.electron === undefined
          ? {}
          : { electron: identityEvidence(value.electron) }),
        ...(value.appAsar === null || value.appAsar === undefined
          ? {}
          : { appAsar: identityEvidence(value.appAsar) }),
        packageLayoutDigest: value.metadata.packageLayoutDigest,
      },
    ]),
  );
}
function identityEvidence(identity) {
  return {
    bytes: identity.bytes,
    sha256: identity.sha256,
    pathSha256: createHash('sha256').update(identity.path).digest('hex'),
  };
}

export function redactEvidence(value, key = '') {
  if (Array.isArray(value)) return value.map((entry) => redactEvidence(entry, key));
  if (value !== null && typeof value === 'object') {
    return Object.fromEntries(
      Object.entries(value).map(([entryKey, entryValue]) => [
        entryKey,
        redactEvidence(entryValue, entryKey),
      ]),
    );
  }
  if (typeof value !== 'string') return value;
  const sensitiveKey =
    /(?:path|root|directory|executable|arguments?|output|pipe|credential|secret|private)/iu.test(
      key,
    );
  const sensitiveValue =
    /^[A-Za-z]:[\\/]/u.test(value) ||
    value.startsWith('\\\\.\\pipe\\') ||
    /-----BEGIN [A-Z ]*PRIVATE KEY-----/u.test(value);
  if (!sensitiveKey && !sensitiveValue) return value;
  return {
    redacted: true,
    sha256: createHash('sha256').update(value).digest('hex'),
  };
}

async function persist(fileSystem, path, evidence) {
  await fileSystem.mkdir(resolve(path, '..'));
  await fileSystem.writeFile(path, `${JSON.stringify(redactEvidence(evidence), null, 2)}\n`);
}

export function nodeFileSystem() {
  return {
    readFile,
    lstat,
    mkdir: (path) => mkdir(path, { recursive: true }),
    writeFile: (path, value) => writeFile(path, value, { encoding: 'utf8', mode: 0o600 }),
  };
}

export function createWindowsAcceptanceRunner(
  plan,
  osAdapter,
  adapterFactory = createWindowsOsAdapter,
) {
  return createProductionRunner(plan, osAdapter ?? adapterFactory(plan.acceptance));
}

async function main() {
  const evidencePath = required('--evidence');
  const input = JSON.parse(await readFile(resolve(evidencePath), 'utf8'));
  const plan = await createInstalledAcceptancePlan(input);
  const execute = process.argv.includes('--execute');
  const result = await executeInstalledAcceptance(
    plan,
    { fileSystem: nodeFileSystem(), runner: createWindowsAcceptanceRunner(plan) },
    { dryRun: !execute },
  );
  console.log(JSON.stringify(result, null, 2));
}
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
function required(name) {
  const value = valueAfter(name);
  if (!value) throw new Error(`Missing ${name}`);
  return value;
}
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
