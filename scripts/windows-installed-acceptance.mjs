import { createHash } from 'node:crypto';
import { lstat, mkdir, open, readFile, readdir, writeFile } from 'node:fs/promises';
import { basename, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';
import { SOURCE_COMMIT_MARKER, SOURCE_TREE_MARKER } from './helper-build-contract.mjs';
import {
  assertNoLinkPath,
  resolveBundlePath,
  verifyAcceptanceBundleTree,
} from './windows-installed-acceptance-bundle.mjs';
import { parseNativeArchitectures } from './native-architecture.mjs';
import {
  inspectWindowsUpdaterKey,
  validatePackageReleaseMetadata,
  verifyWindowsUpdaterReleaseBinding,
} from './release-package-metadata.mjs';
import { bindTqpkg2OwnerManifest, parseTqpkg2 } from './tqpkg2.mjs';
import { verifyFaultEvidenceChain } from './windows-installed-acceptance-fault-evidence.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_PHASE_SCHEDULE,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_REQUEST_MS,
  MAX_ACCEPTANCE_RUN_MS,
} from './windows-installed-acceptance-schedule.mjs';
import {
  authenticatedUpdateBootstrapArgument,
  createProductionRunner,
  createWindowsOsAdapter,
  externalTimeout,
  runProductionPhase,
  startTrustedAcceptanceBroker,
} from './windows-installed-acceptance-runner.mjs';

export {
  authenticatedUpdateBootstrapArgument,
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
export {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_PHASE_SCHEDULE,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_REQUEST_MS,
  MAX_ACCEPTANCE_RUN_MS,
};

export class AcceptanceStoppedError extends Error {
  constructor(message, evidence) {
    super(message);
    this.name = 'AcceptanceStoppedError';
    this.evidence = evidence;
  }
}

export async function createInstalledAcceptancePlan(
  input,
  fileSystem = nodeFileSystem(),
  options = {},
) {
  if (!['x64', 'arm64'].includes(input.architecture)) {
    throw new Error('Acceptance requires an exact native x64 or arm64 architecture');
  }
  const artifacts = {};
  for (const name of ['predecessor', 'candidate', 'fresh', 'repair', 'fault']) {
    await options.reverifyBundle?.();
    artifacts[name] = await freezeArtifact(
      name,
      input.artifacts?.[name],
      input.architecture,
      fileSystem,
    );
  }
  if (input.artifacts?.faults !== undefined) {
    artifacts.faults = {};
    for (const phase of ACCEPTANCE_FAULT_PHASES) {
      const faultInput = input.artifacts.faults[phase];
      if (faultInput === undefined)
        throw new Error(`Acceptance fault artifact is missing for ${phase}`);
      await options.reverifyBundle?.();
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
  if (artifacts.faults !== undefined) {
    verifyFaultEvidenceChain(
      ACCEPTANCE_FAULT_PHASES.map((phase) => ({
        bytes: artifacts.faults[phase].validationEvidence.content,
        artifact: artifacts.faults[phase],
      })),
      {
        buildId: input.acceptance?.buildId,
        architecture: input.architecture,
        sourceCommit: candidate.sourceCommit,
        sourceTree: candidate.sourceTree,
        candidateSha256: artifacts.candidate.installer.sha256,
        candidateLayoutDigest: candidate.packageLayoutDigest,
        publicKeySpkiBase64url: input.acceptance?.validationPublicKeySpkiBase64url,
        chainHeadSha256: input.acceptance?.validationChainHeadSha256,
      },
    );
  }
  const predecessorUpdaterKey = inspectWindowsUpdaterKey(
    resolve(artifacts.predecessor.unpackedRoot, role(predecessor, 'gateway').path),
    input.architecture,
  );
  verifyWindowsUpdaterReleaseBinding(
    artifacts.candidate.releaseIdentity,
    predecessorUpdaterKey.sec1,
  );
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
  if (
    fresh.packageMode !== 'fresh' ||
    fresh.freshInstall !== true ||
    fresh.predecessor !== null ||
    predecessor.packageMode !== 'fresh' ||
    predecessor.freshInstall !== true ||
    predecessor.predecessor !== null ||
    artifacts.fresh.installer.sha256 !== artifacts.predecessor.installer.sha256 ||
    artifacts.fresh.installer.bytes !== artifacts.predecessor.installer.bytes ||
    canonicalAcceptanceJson(fresh) !== canonicalAcceptanceJson(predecessor)
  ) {
    throw new Error('Canonical predecessor and reinstall artifacts are not exactly equal');
  }
  assertAcceptanceCandidateLineage(predecessor, candidate);
  if (
    artifacts.repair.packageManifest?.packageMode !== 'repair' ||
    artifacts.repair.packageManifest.predecessor !== null ||
    canonicalTargetIdentity(repair) !== canonicalTargetIdentity(candidate) ||
    artifacts.repair.packageManifest.target.releaseBuildDigest !== candidate.releaseBuildDigest ||
    artifacts.repair.packageManifest.target.gatewaySha256 !== role(candidate, 'gateway').sha256 ||
    artifacts.repair.packageManifest.target.ownerSha256 !== role(candidate, 'owner').sha256
  ) {
    throw new Error('Repair artifact is not bound to the exact candidate identity');
  }
  const faultArtifacts = artifacts.faults ?? { published: artifacts.fault };
  for (const [phase, artifact] of Object.entries(faultArtifacts)) {
    if (
      artifact.packageManifest?.faultPhase !== phase ||
      canonicalTargetIdentity(artifact.metadata) !== canonicalTargetIdentity(candidate)
    ) {
      throw new Error(`Fault-injection artifact is not bound to the exact candidate: ${phase}`);
    }
  }
  await options.reverifyBundle?.();
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
  const buildManifest = buildManifestFile.content.toString('utf8').trim();
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
  const signedRequests = JSON.parse(requestsFile.content.toString('utf8'));
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
    syntheticSender: await freezeAcceptanceExecutable(
      'Synthetic sender',
      input.acceptance?.syntheticSenderPath,
      input.acceptance?.syntheticSenderSha256,
      input.architecture,
      candidate,
      fileSystem,
    ),
    trustedLauncher: await freezeAcceptanceExecutable(
      'Trusted launcher',
      input.acceptance?.trustedLauncherPath,
      input.acceptance?.trustedLauncherSha256,
      input.architecture,
      candidate,
      fileSystem,
    ),
    acceptanceBroker: await freezeAcceptanceExecutable(
      'Acceptance broker',
      input.acceptance?.acceptanceBrokerPath,
      input.acceptance?.acceptanceBrokerSha256,
      input.architecture,
      candidate,
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
    canonicalRelease:
      input.canonicalRelease === undefined
        ? undefined
        : await freezeCanonicalRelease(input.canonicalRelease, artifacts, fileSystem),
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
  const now =
    typeof options.nowMs === 'function' ? options.nowMs : () => options.nowMs ?? Date.now();
  const preflightNowMs = now();
  const sequence = validateAcceptanceRunSequence(plan.acceptance, preflightNowMs);
  if (typeof runner.preflightAcceptance !== 'function') {
    throw new Error('Acceptance runner preflight is unavailable');
  }
  const dryRun = options.dryRun !== false;
  if (!dryRun && (runner.platform !== 'win32' || runner.architecture !== plan.architecture)) {
    throw new Error('Installed acceptance requires an exact native Windows host');
  }
  const preflight = await runner.preflightAcceptance({
    plan,
    sequence,
    nowMs: preflightNowMs,
    reserveNonces: !dryRun,
  });
  if (dryRun) {
    return Object.freeze({
      result: 'dry-run',
      validation: redactEvidence(preflight),
      architecture: plan.architecture,
      matrix: plan.matrix,
      artifactHashes: artifactEvidence(plan.artifacts),
    });
  }
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
    JSON.parse(metadataIdentity.content.toString('utf8')),
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
  const nativePackage = parseTqpkg2(installer.content, metadata.architecture, {
    allowAcceptanceFaults: name === 'fault',
    allowAcceptanceRepair: name === 'repair' || name === 'fault',
  });
  if (name === 'repair' || name === 'fault') {
    bindAcceptanceRepairTarget(nativePackage.manifest, metadata);
  } else {
    bindTqpkg2OwnerManifest(nativePackage.manifest, metadata);
  }
  await bindTqpkg2UnpackedTree(name, nativePackage, unpackedRoot, fileSystem);
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
    releaseIdentity = JSON.parse(releaseIdentityFile.content.toString('utf8'));
    if (
      releaseIdentity.packageSha256 !== installer.sha256 ||
      releaseIdentity.packageLayoutDigest !== metadata.packageLayoutDigest ||
      releaseIdentity.version !== metadata.version ||
      releaseIdentity.platform !== metadata.platform ||
      releaseIdentity.architecture !== metadata.architecture ||
      releaseIdentity.packageMode !== metadata.packageMode ||
      releaseIdentity.sourceCommit !== metadata.sourceCommit ||
      releaseIdentity.sourceTree !== metadata.sourceTree ||
      releaseIdentity.releaseBuildDigest !== metadata.releaseBuildDigest ||
      canonicalAcceptanceJson(releaseIdentity.roles) !== canonicalAcceptanceJson(metadata.roles) ||
      canonicalAcceptanceJson(releaseIdentity.predecessor) !==
        canonicalAcceptanceJson(metadata.predecessor)
    ) {
      throw new Error(`${name} release identity does not bind installer and unpacked layout`);
    }
  } else if (name === 'candidate') {
    throw new Error('candidate release identity is required');
  }
  if (
    name !== 'repair' &&
    name !== 'fault' &&
    nativePackage.manifest.packageMode !== metadata.packageMode
  ) {
    throw new Error(`${name} TQPKG2 mode does not match release metadata`);
  }
  let validationEvidence = null;
  if (name === 'fault') {
    validationEvidence = await regularIdentity(
      input.validationEvidencePath,
      input.validationEvidenceSha256,
      fileSystem,
    );
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
    isolatedValidation: validationEvidence !== null,
    validationEvidence,
    packageManifest: nativePackage.manifest,
  });
}

function bindAcceptanceRepairTarget(manifest, metadata) {
  if (
    manifest.packageMode !== 'repair' ||
    manifest.predecessor !== null ||
    manifest.version !== metadata.version ||
    manifest.architecture !== metadata.architecture ||
    manifest.sourceCommit !== metadata.sourceCommit ||
    manifest.sourceTree !== metadata.sourceTree ||
    manifest.target.releaseBuildDigest !== metadata.releaseBuildDigest ||
    manifest.target.gatewaySha256 !== role(metadata, 'gateway').sha256 ||
    manifest.target.ownerSha256 !== role(metadata, 'owner').sha256 ||
    manifest.target.recoveryLauncherSha256 !== role(metadata, 'recovery-launcher').sha256
  ) {
    throw new Error('Acceptance repair wrapper is not bound to its exact candidate tree');
  }
}

async function bindTqpkg2UnpackedTree(name, nativePackage, unpackedRoot, fileSystem) {
  if (typeof fileSystem.readdir !== 'function') {
    throw new Error(`${name} unpacked tree enumeration is unavailable`);
  }
  const actual = [];
  const visit = async (directory) => {
    const entries = await fileSystem.readdir(directory);
    for (const entry of entries) {
      const path = resolve(directory, entry.name);
      const identity = await fileSystem.lstat(path);
      if (identity.isSymbolicLink()) throw new Error(`${name} unpacked tree contains a link`);
      if (identity.isDirectory()) await visit(path);
      else if (identity.isFile()) {
        actual.push(relative(unpackedRoot, path).split(sep).join('/'));
      } else {
        throw new Error(`${name} unpacked tree contains a non-regular entry`);
      }
    }
  };
  await visit(unpackedRoot);
  actual.sort((left, right) => left.localeCompare(right, 'en'));
  const expected = [...nativePackage.contents.keys()].sort((left, right) =>
    left.localeCompare(right, 'en'),
  );
  if (canonicalAcceptanceJson(actual) !== canonicalAcceptanceJson(expected)) {
    throw new Error(`${name} TQPKG2 and unpacked path sets differ`);
  }
  for (const path of expected) {
    const unpacked = await fileSystem.readFile(resolve(unpackedRoot, path));
    if (!unpacked.equals(nativePackage.contents.get(path))) {
      throw new Error(`${name} TQPKG2 and unpacked bytes differ: ${path}`);
    }
  }
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
      payload.expiresAtMs - payload.issuedAtMs > MAX_ACCEPTANCE_REQUEST_MS ||
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

async function freezeAcceptanceExecutable(
  label,
  path,
  expectedSha256,
  architecture,
  sourceIdentity,
  fileSystem,
) {
  const identity = await regularIdentity(path, expectedSha256, fileSystem);
  const native = parseNativeArchitectures(identity.content, identity.path);
  if (
    native?.format !== 'pe' ||
    canonicalAcceptanceJson(native.architectures) !== `[${JSON.stringify(architecture)}]`
  ) {
    throw new Error(`${label} architecture is not exact`);
  }
  verifyRetainedNativeSourceIdentity(identity.content, sourceIdentity, label);
  if (
    label === 'Trusted launcher' &&
    !identity.content.includes(Buffer.from('--windows-installed-acceptance-broker-v1', 'ascii'))
  ) {
    throw new Error('Trusted launcher acceptance broker marker is missing');
  }
  return identity;
}

function verifyRetainedNativeSourceIdentity(bytes, sourceIdentity, label) {
  for (const [marker, expected] of [
    [SOURCE_COMMIT_MARKER, sourceIdentity.sourceCommit],
    [SOURCE_TREE_MARKER, sourceIdentity.sourceTree],
  ]) {
    const offset = bytes.indexOf(marker);
    const value = bytes.subarray(offset + marker.length, offset + marker.length + 40);
    if (
      offset < 0 ||
      bytes.indexOf(marker, offset + 1) >= 0 ||
      value.toString('ascii') !== expected
    ) {
      throw new Error(`${label} source identity is invalid`);
    }
  }
}

async function regularIdentity(path, expectedSha256, fileSystem) {
  if (!/^[0-9a-f]{64}$/u.test(expectedSha256 ?? ''))
    throw new Error(`Invalid expected SHA-256: ${basename(path ?? '')}`);
  const absolute = resolve(path);
  let metadata;
  let bytes;
  if (typeof fileSystem.open === 'function') {
    const handle = await fileSystem.open(absolute, 'r');
    try {
      const before = await handle.stat();
      bytes = await handle.readFile();
      const after = await handle.stat();
      if (
        !before.isFile() ||
        before.nlink !== 1 ||
        before.size !== bytes.length ||
        before.size !== after.size ||
        before.mtimeMs !== after.mtimeMs
      ) {
        throw new Error(`Frozen input changed while read: ${basename(absolute)}`);
      }
      metadata = before;
    } finally {
      await handle.close();
    }
  } else {
    [metadata, bytes] = await Promise.all([
      fileSystem.lstat(absolute),
      fileSystem.readFile(absolute),
    ]);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new Error(`Frozen input is not a regular file: ${basename(absolute)}`);
    }
  }
  const sha256 = createHash('sha256').update(bytes).digest('hex');
  if (sha256 !== expectedSha256)
    throw new Error(`Frozen input SHA-256 mismatch: ${basename(absolute)}`);
  return Object.freeze({ path: absolute, bytes: metadata.size, sha256, content: bytes });
}
function assertAcceptanceCandidateLineage(predecessor, candidate) {
  for (const field of ['version', 'architecture', 'sourceCommit', 'sourceTree']) {
    if (candidate[field] !== predecessor[field]) {
      throw new Error(`Acceptance candidate differs from the canonical predecessor ${field}`);
    }
  }
  if (candidate.platform !== 'win' || predecessor.platform !== 'win') {
    throw new Error('Acceptance candidate lineage must be Windows');
  }
  for (const name of ['owner', 'recovery-launcher']) {
    if (
      canonicalAcceptanceJson(role(candidate, name)) !==
      canonicalAcceptanceJson(role(predecessor, name))
    ) {
      throw new Error(`Acceptance candidate changed the canonical ${name} role`);
    }
  }
  const candidateGateway = role(candidate, 'gateway');
  const predecessorGateway = role(predecessor, 'gateway');
  if (
    candidateGateway.sha256 === predecessorGateway.sha256 ||
    canonicalAcceptanceJson({ ...candidateGateway, sha256: null }) !==
      canonicalAcceptanceJson({ ...predecessorGateway, sha256: null })
  ) {
    throw new Error('Acceptance candidate must contain only the distinct acceptance gateway');
  }
}

async function freezeCanonicalRelease(input, artifacts, fileSystem) {
  const descriptorFile = await regularIdentity(
    input.descriptorPath,
    input.descriptorSha256,
    fileSystem,
  );
  const provenanceFile = await regularIdentity(
    input.provenancePath,
    input.provenanceSha256,
    fileSystem,
  );
  const descriptor = JSON.parse(descriptorFile.content.toString('utf8'));
  const provenance = JSON.parse(provenanceFile.content.toString('utf8'));
  validateArtifactProvenanceManifest(provenance);
  const predecessor = artifacts.predecessor;
  if (
    descriptor.architecture !== predecessor.metadata.architecture ||
    descriptor.sourceCommit !== predecessor.metadata.sourceCommit ||
    descriptor.sourceTree !== predecessor.metadata.sourceTree ||
    descriptor.sha256 !== predecessor.installer.sha256 ||
    provenance.sourceCommit !== descriptor.sourceCommit ||
    provenance.sourceTree !== descriptor.sourceTree ||
    provenance.package?.version !== descriptor.version ||
    provenance.package?.platform !== 'win' ||
    provenance.package?.arch !== descriptor.architecture ||
    artifacts.fresh.installer.sha256 !== descriptor.sha256
  ) {
    throw new Error('Canonical release builder provenance does not bind the frozen predecessor');
  }
  const final = provenance.entries?.filter(
    (entry) => entry.role === 'final-artifact' && basename(entry.path) === descriptor.installer,
  );
  if (
    final?.length !== 1 ||
    final[0].kind !== 'file' ||
    final[0].size !== predecessor.installer.bytes ||
    final[0].sha256 !== predecessor.installer.sha256
  ) {
    throw new Error('Canonical release builder provenance does not bind the installer');
  }
  const prefix = `${provenance.package.root}/`;
  const entries = provenance.entries.filter((entry) => entry.role === 'package-file');
  const files = predecessor.packageManifest.files;
  const byPath = new Map(
    entries.map((entry) => [
      entry.path.startsWith(prefix) ? entry.path.slice(prefix.length) : '',
      entry,
    ]),
  );
  if (byPath.has('') || byPath.size !== entries.length || byPath.size !== files.length) {
    throw new Error('Canonical release builder provenance package inventory is invalid');
  }
  for (const file of files) {
    const entry = byPath.get(file.path);
    if (entry?.kind !== 'file' || entry.size !== file.size || entry.sha256 !== file.sha256) {
      throw new Error(`Canonical release builder provenance package file differs: ${file.path}`);
    }
  }
  return Object.freeze({ descriptor: descriptorFile, provenance: provenanceFile });
}

function canonicalTargetIdentity(metadata) {
  return canonicalAcceptanceJson({
    architecture: metadata.architecture,
    roles: metadata.roles,
    sourceCommit: metadata.sourceCommit,
    sourceTree: metadata.sourceTree,
    version: metadata.version,
  });
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
      value?.installer === undefined
        ? artifactEvidence(value)
        : {
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
    /(?:path|root|directory|executable|arguments?|output|pipe|credential|secret|private|signedRequests?|requestNonce|launchCorrelation|authorization|bearer)/iu.test(
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

export function nodeFileSystem(retainedOutput) {
  return {
    readFile,
    lstat,
    open,
    readdir: (path) => readdir(path, { withFileTypes: true }),
    mkdir: (path) => mkdir(path, { recursive: true }),
    writeFile: async (path, value) => {
      if (retainedOutput !== undefined && resolve(path) === retainedOutput.path) {
        const bytes = Buffer.from(value, 'utf8');
        await retainedOutput.handle.truncate(0);
        await retainedOutput.handle.write(bytes, 0, bytes.length, 0);
        await retainedOutput.handle.sync();
        return;
      }
      await writeFile(path, value, { encoding: 'utf8', mode: 0o600 });
    },
  };
}

export function createWindowsAcceptanceRunner(
  plan,
  osAdapter,
  adapterFactory = createWindowsOsAdapter,
) {
  return createProductionRunner(plan, osAdapter ?? adapterFactory(plan.acceptance));
}

export function resolveInstalledAcceptanceInputPaths(input, evidencePath) {
  const base = resolve(evidencePath, '..');
  const copy = structuredClone(input);
  const pathFields = [
    'installerPath',
    'metadataPath',
    'releaseIdentityPath',
    'validationEvidencePath',
    'unpackedRoot',
    'electronPath',
    'appAsarPath',
  ];
  const artifacts = [
    copy.artifacts?.predecessor,
    copy.artifacts?.candidate,
    copy.artifacts?.fresh,
    copy.artifacts?.repair,
    copy.artifacts?.fault,
    ...Object.values(copy.artifacts?.faults ?? {}),
  ];
  for (const artifact of artifacts) {
    if (artifact === undefined) continue;
    for (const field of pathFields) {
      if (typeof artifact[field] === 'string') {
        artifact[field] = resolveBundlePath(base, artifact[field]);
      }
    }
  }
  for (const field of ['descriptorPath', 'provenancePath']) {
    if (typeof copy.canonicalRelease?.[field] === 'string') {
      copy.canonicalRelease[field] = resolveBundlePath(base, copy.canonicalRelease[field]);
    }
  }
  for (const field of [
    'buildManifestPath',
    'signedRequestsPath',
    'syntheticSenderPath',
    'acceptanceBrokerPath',
    'trustedLauncherPath',
  ]) {
    if (typeof copy.acceptance?.[field] === 'string') {
      copy.acceptance[field] = resolveBundlePath(base, copy.acceptance[field]);
    }
  }
  if (copy.outputPath !== '../evidence.json') {
    throw new Error('Acceptance evidence output must use the fixed bundle-relative path');
  }
  copy.outputPath = resolve(base, copy.outputPath);
  return copy;
}

async function main() {
  const evidencePath = resolve(required('--evidence'));
  const bundleRoot = resolve(required('--bundle-root'));
  if (resolve(evidencePath, '..') !== bundleRoot) {
    throw new Error('Acceptance evidence must be the direct child of the frozen bundle root');
  }
  const acceptanceRoot = resolve('tmp/windows-installed-acceptance');
  if (relative(acceptanceRoot, bundleRoot) !== 'frozen') {
    throw new Error('Frozen acceptance bundle root is not canonical');
  }
  await assertNoLinkPath(bundleRoot, { directory: true });
  const manifestSha256 = process.env.ACCEPTANCE_MANIFEST_SHA256;
  if (!/^[0-9a-f]{64}$/u.test(manifestSha256 ?? '')) {
    throw new Error('ACCEPTANCE_MANIFEST_SHA256 is required');
  }
  const bundleExpectation = { manifestSha256 };
  await verifyAcceptanceBundleTree(bundleRoot, bundleExpectation);
  const rawInput = JSON.parse(await readFile(evidencePath, 'utf8'));
  const input = resolveInstalledAcceptanceInputPaths(rawInput, evidencePath);
  const outputPath = resolve(required('--output'));
  if (outputPath !== input.outputPath || resolve(outputPath, '..') !== acceptanceRoot) {
    throw new Error('Acceptance evidence output escapes its controlled directory');
  }
  await assertNoLinkPath(resolve(outputPath, '..'), { directory: true });
  const execute = process.argv.includes('--execute');
  const outputHandle = execute ? await open(outputPath, 'wx', 0o600) : null;
  if (outputHandle !== null) {
    try {
      await assertNoLinkPath(outputPath, { file: true });
    } catch (error) {
      await outputHandle.close();
      throw error;
    }
  }
  const fileSystem = nodeFileSystem(
    outputHandle === null ? undefined : { path: outputPath, handle: outputHandle },
  );
  try {
    const reverifyBundle = () => verifyAcceptanceBundleTree(bundleRoot, bundleExpectation);
    const plan = await createInstalledAcceptancePlan(input, fileSystem, { reverifyBundle });
    const result = await executeInstalledAcceptance(
      plan,
      { fileSystem, runner: createWindowsAcceptanceRunner(plan) },
      { dryRun: !execute },
    );
    console.log(JSON.stringify(result, null, 2));
  } finally {
    await outputHandle?.close();
  }
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
