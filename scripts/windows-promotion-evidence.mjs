import {
  createHash,
  createPrivateKey,
  createPublicKey,
  sign as signBytes,
  verify as verifyBytes,
} from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalJson } from './release-manifest.mjs';
import { verifyAuthenticatedSetupReceipt } from './windows-installer-success-evidence.mjs';

const SHA256 = /^[0-9a-f]{64}$/u;
const SOURCE = /^[0-9a-f]{40}$/u;
const GENERATION = /^[1-9][0-9]*$/u;
const DOMAIN = Buffer.from('TalkingQuill/windows-promotion-lifecycle-evidence/v4\0');
const SUCCESS_NAMES = (arch) => [
  [`windows-installer-success-fresh-${arch}.json`, 'fresh', 'install'],
];
const MIGRATION_NAME = (arch) => `windows-local-migration-${arch}.json`;
const TERMINAL_FAULT_NAME = (arch) => `windows-terminal-fault-candidate-${arch}.json`;
const REBOOT_NAME = (arch) => `windows-reboot-acceptance-${arch}.json`;
const INSTALLED_ACCEPTANCE_NAME = 'windows-installed-acceptance-x64-gate.json';

function exactObject(value, keys, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    throw new Error(`${label} must be an object`);
  const actual = Object.keys(value).sort();
  const expected = [...keys].sort();
  if (actual.length !== expected.length || actual.some((key, index) => key !== expected[index]))
    throw new Error(`${label} has an unexpected schema`);
}

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function publicSec1(key) {
  const jwk = createPublicKey(key).export({ format: 'jwk' });
  return Buffer.concat([
    Buffer.from([4]),
    Buffer.from(jwk.x, 'base64url'),
    Buffer.from(jwk.y, 'base64url'),
  ]);
}

function validateIdentity(identity, label) {
  exactObject(
    identity,
    [
      'pid',
      'parentPid',
      'sessionId',
      'creationUtcTicks',
      'imagePath',
      'imageSha256',
      'userSid',
      'logonId',
    ],
    label,
  );
  if (
    !Number.isSafeInteger(identity.pid) ||
    identity.pid <= 0 ||
    !Number.isSafeInteger(identity.parentPid) ||
    identity.parentPid < 0 ||
    !Number.isSafeInteger(identity.sessionId) ||
    identity.sessionId < 0 ||
    !Number.isSafeInteger(identity.creationUtcTicks) ||
    identity.creationUtcTicks <= 0 ||
    typeof identity.imagePath !== 'string' ||
    identity.imagePath.length === 0 ||
    !SHA256.test(identity.imageSha256) ||
    typeof identity.userSid !== 'string' ||
    identity.userSid.length === 0 ||
    typeof identity.logonId !== 'string' ||
    identity.logonId.length === 0
  )
    throw new Error(`${label} is invalid`);
}

function validateProductionInterruptions(value, arch) {
  exactObject(
    value,
    ['fresh', 'removeFreshRecovery', 'uninstall', 'finishUninstallRecovery'],
    `${arch} production interruption evidence`,
  );
  if (
    value.fresh !== true ||
    value.removeFreshRecovery !== true ||
    value.uninstall !== true ||
    value.finishUninstallRecovery !== true
  )
    throw new Error(`${arch} production interruption evidence is invalid`);
}

function validateSuccess(value, arch, operation, action) {
  const required = [
    'schemaVersion',
    'installer',
    'installerSha256',
    'architecture',
    'sourceCommit',
    'sourceTree',
    'packageMode',
    'operation',
    'targetReleaseBuildDigest',
    'targetGatewaySha256',
    'targetOwnerSha256',
    'controllerPid',
    'authenticatedSetupPids',
    'processIdentities',
    'pipeObserved',
    'nativeAuthenticationReceipt',
    'installedIdentityBound',
    'registrationsExact',
    'terminalTopology',
    'exitCode',
    'interpreterProcessStarts',
    'observerErrors',
    'passed',
  ];
  if (operation === 'fresh') required.push('productionInterruptions');
  exactObject(value, required, `${arch} ${operation} evidence`);
  if (
    value.schemaVersion !== 2 ||
    value.architecture !== arch ||
    value.operation !== operation ||
    value.packageMode !== (operation === 'fresh' ? 'fresh' : 'update') ||
    value.passed !== true ||
    value.exitCode !== 0 ||
    value.pipeObserved !== true ||
    value.installedIdentityBound !== true ||
    value.registrationsExact !== true ||
    value.terminalTopology !== true ||
    !SHA256.test(value.installerSha256) ||
    !SHA256.test(value.targetReleaseBuildDigest) ||
    !SHA256.test(value.targetGatewaySha256) ||
    !SHA256.test(value.targetOwnerSha256) ||
    !SOURCE.test(value.sourceCommit) ||
    !SOURCE.test(value.sourceTree) ||
    !Array.isArray(value.interpreterProcessStarts) ||
    value.interpreterProcessStarts.length !== 0 ||
    !Array.isArray(value.observerErrors) ||
    value.observerErrors.length !== 0 ||
    !Array.isArray(value.processIdentities) ||
    value.processIdentities.length !== 2 ||
    !Array.isArray(value.authenticatedSetupPids) ||
    value.authenticatedSetupPids.length !== 2
  )
    throw new Error(`${arch} ${operation} evidence did not pass exact lifecycle policy`);
  if (operation === 'fresh') validateProductionInterruptions(value.productionInterruptions, arch);
  verifyAuthenticatedSetupReceipt(value.nativeAuthenticationReceipt, action, value.installerSha256);
  value.processIdentities.forEach((identity, index) =>
    validateIdentity(identity, `${arch} ${operation} process ${index}`),
  );
  const controller = value.processIdentities.find(({ pid }) => pid === value.controllerPid);
  const worker = value.processIdentities.find(({ parentPid }) => parentPid === value.controllerPid);
  if (!controller || !worker || controller.pid === worker.pid)
    throw new Error(`${arch} ${operation} kernel process lineage is invalid`);
  return {
    kind: 'success',
    architecture: arch,
    operation,
    action,
    passed: value.passed,
    exitCode: value.exitCode,
    packageSha256: value.installerSha256,
    releaseBuildDigest: value.targetReleaseBuildDigest,
    layoutDigest: value.targetReleaseBuildDigest,
    sourceCommit: value.sourceCommit,
    sourceTree: value.sourceTree,
    processTokenImageKernelObservations: value.processIdentities,
    peerObservation: {
      pipeObserved: value.pipeObserved,
      authenticatedSetupPids: value.authenticatedSetupPids,
      receipt: value.nativeAuthenticationReceipt,
    },
    installedState: {
      installedIdentityBound: value.installedIdentityBound,
      registrationsExact: value.registrationsExact,
      terminalTopology: value.terminalTopology,
      gatewaySha256: value.targetGatewaySha256,
      ownerSha256: value.targetOwnerSha256,
    },
    faultPhase: null,
    faultResult: null,
    evidence: value,
  };
}

function validateArchitectureBinding(records, arch) {
  const claims = records
    .filter(({ claims: record }) => record.architecture === arch)
    .map(({ claims: record }) => record);
  const fresh = claims.find(({ operation }) => operation === 'fresh');
  const migration = claims.find(({ operation }) => operation === 'local-uninstall-preserve-fresh');
  const terminalFault = claims.find(({ operation }) => operation === 'reboot-pending-delete');
  const reboot = claims.find(({ terminalFaultClassification }) => terminalFaultClassification);
  if (
    fresh === undefined ||
    migration === undefined ||
    terminalFault === undefined ||
    reboot === undefined ||
    migration.packageSha256 !== fresh.packageSha256 ||
    migration.releaseBuildDigest !== fresh.releaseBuildDigest ||
    migration.sourceCommit !== fresh.sourceCommit ||
    migration.sourceTree !== fresh.sourceTree ||
    migration.localProvenance !== 'local-non-public' ||
    terminalFault.packageSha256 === fresh.packageSha256 ||
    reboot.terminalFaultCandidateSha256 !== terminalFault.packageSha256 ||
    reboot.recoveryFreshCandidateSha256 !== fresh.packageSha256
  )
    throw new Error(`${arch} promotion evidence generation binding is invalid`);
}

function validateInventory(value, label) {
  if (
    !Array.isArray(value) ||
    value.length === 0 ||
    value.some(
      (entry) =>
        entry === null ||
        typeof entry !== 'object' ||
        Object.keys(entry).sort().join(',') !== 'path,sha256' ||
        typeof entry.path !== 'string' ||
        entry.path.length === 0 ||
        !SHA256.test(entry.sha256),
    ) ||
    new Set(value.map(({ path }) => path)).size !== value.length
  )
    throw new Error(`${label} inventory is invalid`);
}

function validateLocalMigration(source, arch, freshClaims) {
  const value = JSON.parse(source);
  exactObject(
    value,
    [
      'schemaVersion',
      'mode',
      'provenance',
      'architecture',
      'sourceVersion',
      'targetVersion',
      'sourceCommit',
      'sourceTree',
      'baselineWorkflowRunId',
      'baselineArtifactDigest',
      'localInstallerSha256',
      'installedManifestSha256',
      'installedManifestUtf8Base64',
      'installedManifest',
      'installedReleaseBuildDigest',
      'installedGatewaySha256',
      'installedOwnerSha256',
      'updaterMarkerPresent',
      'profileInventoryBefore',
      'profileInventoryAfterUninstall',
      'profileInventoryAfterFresh',
      'modelInventoryBefore',
      'modelInventoryAfterUninstall',
      'modelInventoryAfterFresh',
      'sentinelInventoryBefore',
      'sentinelInventoryAfterUninstall',
      'sentinelInventoryAfterFresh',
      'machineQuitObserved',
      'singletonReleased',
      'uninstallExitCode',
      'uninstallResidue',
      'freshInstallExitCode',
      'targetInstallerSha256',
      'targetReleaseBuildDigest',
      'targetGatewaySha256',
      'targetOwnerSha256',
      'targetMaintenanceGeneration',
      'passed',
    ],
    `${arch} local migration evidence`,
  );
  const manifestBytes = Buffer.from(value.installedManifestUtf8Base64 ?? '', 'base64');
  let decodedManifest;
  try {
    decodedManifest = JSON.parse(manifestBytes.toString('utf8'));
  } catch {
    throw new Error(`${arch} preserved installed manifest is invalid`);
  }
  const manifest = value.installedManifest;
  const roles = manifest?.roles;
  const gateway = Array.isArray(roles) ? roles.find(({ role }) => role === 'gateway') : undefined;
  const owner = Array.isArray(roles) ? roles.find(({ role }) => role === 'owner') : undefined;
  for (const [inventory, label] of [
    [value.profileInventoryBefore, 'profile before'],
    [value.profileInventoryAfterUninstall, 'profile after uninstall'],
    [value.profileInventoryAfterFresh, 'profile after fresh'],
    [value.modelInventoryBefore, 'model before'],
    [value.modelInventoryAfterUninstall, 'model after uninstall'],
    [value.modelInventoryAfterFresh, 'model after fresh'],
    [value.sentinelInventoryBefore, 'sentinel before'],
    [value.sentinelInventoryAfterUninstall, 'sentinel after uninstall'],
    [value.sentinelInventoryAfterFresh, 'sentinel after fresh'],
  ])
    validateInventory(inventory, `${arch} ${label}`);
  if (
    value.schemaVersion !== 1 ||
    value.mode !== 'local-uninstall-preserve-fresh' ||
    value.provenance !== 'local-non-public' ||
    value.architecture !== arch ||
    value.sourceVersion !== '0.0.67' ||
    value.targetVersion !== '0.0.69' ||
    value.sourceCommit !== freshClaims.sourceCommit ||
    value.sourceTree !== freshClaims.sourceTree ||
    !GENERATION.test(value.baselineWorkflowRunId) ||
    !/^sha256:[0-9a-f]{64}$/u.test(value.baselineArtifactDigest) ||
    !SHA256.test(value.localInstallerSha256) ||
    !SHA256.test(value.installedManifestSha256) ||
    manifestBytes.length === 0 ||
    hash(manifestBytes) !== value.installedManifestSha256 ||
    canonicalJson(decodedManifest) !== canonicalJson(manifest) ||
    manifest?.version !== '0.0.67' ||
    manifest?.platform !== 'win' ||
    manifest?.architecture !== arch ||
    manifest?.releaseBuildDigest !== value.installedReleaseBuildDigest ||
    gateway?.sha256 !== value.installedGatewaySha256 ||
    owner?.sha256 !== value.installedOwnerSha256 ||
    !SHA256.test(value.installedReleaseBuildDigest) ||
    !SHA256.test(value.installedGatewaySha256) ||
    !SHA256.test(value.installedOwnerSha256) ||
    value.updaterMarkerPresent !== false ||
    canonicalJson(value.profileInventoryBefore) !==
      canonicalJson(value.profileInventoryAfterUninstall) ||
    canonicalJson(value.profileInventoryBefore) !==
      canonicalJson(value.profileInventoryAfterFresh) ||
    canonicalJson(value.modelInventoryBefore) !==
      canonicalJson(value.modelInventoryAfterUninstall) ||
    canonicalJson(value.modelInventoryBefore) !== canonicalJson(value.modelInventoryAfterFresh) ||
    canonicalJson(value.sentinelInventoryBefore) !==
      canonicalJson(value.sentinelInventoryAfterUninstall) ||
    canonicalJson(value.sentinelInventoryBefore) !==
      canonicalJson(value.sentinelInventoryAfterFresh) ||
    value.machineQuitObserved !== true ||
    value.singletonReleased !== true ||
    value.uninstallExitCode !== 0 ||
    !Array.isArray(value.uninstallResidue) ||
    value.uninstallResidue.length !== 0 ||
    value.freshInstallExitCode !== 0 ||
    value.targetInstallerSha256 !== freshClaims.packageSha256 ||
    value.targetReleaseBuildDigest !== freshClaims.releaseBuildDigest ||
    value.targetGatewaySha256 !== freshClaims.installedState?.gatewaySha256 ||
    value.targetOwnerSha256 !== freshClaims.installedState?.ownerSha256 ||
    !/^[0-9a-f]{32}$/u.test(value.targetMaintenanceGeneration) ||
    value.passed !== true
  )
    throw new Error(`${arch} local-uninstall-preserve-fresh evidence is invalid`);
  return {
    kind: 'local-migration',
    architecture: arch,
    operation: value.mode,
    action: 'uninstall-preserve-fresh',
    passed: true,
    exitCode: 0,
    packageSha256: value.targetInstallerSha256,
    releaseBuildDigest: value.targetReleaseBuildDigest,
    layoutDigest: value.targetReleaseBuildDigest,
    sourceCommit: value.sourceCommit,
    sourceTree: value.sourceTree,
    localProvenance: value.provenance,
    baselineWorkflowRunId: value.baselineWorkflowRunId,
    baselineArtifactDigest: value.baselineArtifactDigest,
    localInstallerSha256: value.localInstallerSha256,
    installedManifestSha256: value.installedManifestSha256,
    installedGatewaySha256: value.installedGatewaySha256,
    installedOwnerSha256: value.installedOwnerSha256,
    targetMaintenanceGeneration: value.targetMaintenanceGeneration,
  };
}

function validateTerminalFaultDescriptor(source, arch, freshClaims) {
  const value = JSON.parse(source);
  exactObject(
    value,
    ['schemaVersion', 'architecture', 'classification', 'sha256', 'sourceCommit', 'sourceTree'],
    `${arch} terminal fault descriptor`,
  );
  if (
    value.schemaVersion !== 1 ||
    value.architecture !== arch ||
    value.classification !== 'nonpromotable-acceptance-fault' ||
    !SHA256.test(value.sha256) ||
    value.sha256 === freshClaims.packageSha256 ||
    value.sourceCommit !== freshClaims.sourceCommit ||
    value.sourceTree !== freshClaims.sourceTree
  )
    throw new Error(`${arch} terminal fault descriptor is invalid`);
  return value;
}

function validateRebootEvidence(
  source,
  arch,
  terminalFault,
  freshClaims,
  pinned,
  expectedWorkflowRunId,
) {
  const envelope = JSON.parse(source);
  exactObject(
    envelope,
    ['schemaVersion', 'payload', 'publicKeySha256', 'signature'],
    `${arch} reboot evidence`,
  );
  const payload = envelope.payload;
  exactObject(
    payload,
    [
      'schemaVersion',
      'architecture',
      'terminalFaultCandidateSha256',
      'recoveryFreshCandidateSha256',
      'sourceRevision',
      'sourceTree',
      'workflowRunId',
      'workflowRunAttempt',
      'checkpointSha256',
      'runnerLabel',
      'runnerName',
      'machineIdentity',
      'preBootIdentity',
      'postBootIdentity',
      'rebootRequestMethod',
      'rebootRequestDelaySeconds',
      'rebootRequestAcceptedAt',
      'rebootRequestExitCode',
      'generationBefore',
      'generationAfter',
      'terminalGeneration',
      'serviceImage',
      'pendingDeleteSources',
      'windowsConsumedPendingDeletes',
    ],
    `${arch} reboot evidence payload`,
  );
  const spkiPrefix = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');
  const publicKey = createPublicKey({
    key: Buffer.concat([spkiPrefix, pinned]),
    format: 'der',
    type: 'spki',
  });
  const keyHash = hash(publicKey.export({ format: 'der', type: 'spki' }));
  const signature = Buffer.from(envelope.signature, 'base64url');
  const signed = Buffer.concat([
    Buffer.from('TalkingQuill/windows-real-reboot-acceptance/v1\0'),
    Buffer.from(canonicalJson(payload)),
  ]);
  if (
    envelope.schemaVersion !== 1 ||
    payload.schemaVersion !== 1 ||
    payload.architecture !== arch ||
    payload.terminalFaultCandidateSha256 !== terminalFault.sha256 ||
    payload.recoveryFreshCandidateSha256 !== freshClaims.packageSha256 ||
    payload.terminalFaultCandidateSha256 === payload.recoveryFreshCandidateSha256 ||
    payload.sourceRevision !== terminalFault.sourceCommit ||
    payload.sourceTree !== terminalFault.sourceTree ||
    payload.workflowRunId !== expectedWorkflowRunId ||
    !Number.isSafeInteger(payload.workflowRunAttempt) ||
    payload.workflowRunAttempt < 1 ||
    !SHA256.test(payload.checkpointSha256) ||
    !/^tq-reboot-(x64|arm64)-[a-z0-9-]+$/u.test(payload.runnerLabel) ||
    typeof payload.runnerName !== 'string' ||
    payload.runnerName.length === 0 ||
    envelope.publicKeySha256 !== keyHash ||
    typeof payload.machineIdentity !== 'string' ||
    payload.machineIdentity.length === 0 ||
    payload.preBootIdentity === payload.postBootIdentity ||
    payload.rebootRequestMethod !== 'shutdown.exe' ||
    payload.rebootRequestDelaySeconds !== 30 ||
    payload.rebootRequestExitCode !== 0 ||
    typeof payload.rebootRequestAcceptedAt !== 'string' ||
    !Number.isFinite(Date.parse(payload.rebootRequestAcceptedAt)) ||
    !/^[0-9a-f]{32}$/u.test(payload.generationBefore) ||
    !/^[0-9a-f]{32}$/u.test(payload.generationAfter) ||
    payload.generationBefore === payload.generationAfter ||
    !/^[0-9a-f]{32}$/u.test(payload.terminalGeneration) ||
    typeof payload.serviceImage !== 'string' ||
    payload.serviceImage.length === 0 ||
    !Array.isArray(payload.pendingDeleteSources) ||
    payload.pendingDeleteSources.length === 0 ||
    payload.pendingDeleteSources.some(
      (path) =>
        typeof path !== 'string' ||
        !path.endsWith(`.Talking Quill Terminal Cleanup-${payload.terminalGeneration}.exe`),
    ) ||
    new Set(payload.pendingDeleteSources).size !== payload.pendingDeleteSources.length ||
    !payload.pendingDeleteSources.some((path) => {
      const normalized = path.startsWith('\\??\\') ? path.slice(4) : path;
      return normalized.toLowerCase() === payload.serviceImage.toLowerCase();
    }) ||
    payload.windowsConsumedPendingDeletes !== true ||
    signature.length !== 64 ||
    !verifyBytes('sha256', signed, { key: publicKey, dsaEncoding: 'ieee-p1363' }, signature)
  )
    throw new Error(`${arch} real reboot acceptance evidence is invalid`);
  return {
    ...payload,
    sourceCommit: terminalFault.sourceCommit,
    sourceTree: terminalFault.sourceTree,
    terminalFaultClassification: terminalFault.classification,
  };
}

function validateInstalledAcceptanceGate(source, expectedRunId) {
  const value = JSON.parse(source);
  exactObject(
    value,
    [
      'architecture',
      'artifactSha256',
      'bootstrapSha256',
      'brokerSha256',
      'buildId',
      'bundleSha256',
      'candidateInstallerSha256',
      'evidenceSha256',
      'launcherSha256',
      'phaseCount',
      'producerBundleSha256',
      'producerE2eSha256',
      'purpose',
      'repository',
      'result',
      'runId',
      'schemaVersion',
      'sourceCommit',
      'sourceTree',
      'targetGatewaySha256',
      'targetOwnerSha256',
      'targetPackageLayoutDigest',
      'targetReleaseBuildDigest',
      'validationKeySha256',
      'workflow',
    ],
    'installed acceptance gate',
  );
  if (
    value.schemaVersion !== 1 ||
    value.purpose !== 'talking-quill/windows-installed-acceptance-gate' ||
    value.result !== 'passed' ||
    value.architecture !== 'x64' ||
    value.runId !== expectedRunId ||
    value.workflow !== '.github/workflows/windows-installed-acceptance.yml' ||
    value.phaseCount !== 19 ||
    !SOURCE.test(value.sourceCommit) ||
    !SOURCE.test(value.sourceTree) ||
    !SHA256.test(value.buildId) ||
    !SHA256.test(value.bootstrapSha256) ||
    !SHA256.test(value.brokerSha256) ||
    !SHA256.test(value.launcherSha256) ||
    !SHA256.test(value.bundleSha256) ||
    !SHA256.test(value.candidateInstallerSha256) ||
    !SHA256.test(value.evidenceSha256) ||
    !SHA256.test(value.producerBundleSha256) ||
    !SHA256.test(value.producerE2eSha256) ||
    !SHA256.test(value.targetReleaseBuildDigest) ||
    !SHA256.test(value.targetPackageLayoutDigest) ||
    !SHA256.test(value.targetGatewaySha256) ||
    !SHA256.test(value.targetOwnerSha256) ||
    !SHA256.test(value.validationKeySha256) ||
    !Array.isArray(value.artifactSha256) ||
    value.artifactSha256.some((digest) => !SHA256.test(digest))
  ) {
    throw new Error('Installed acceptance gate is invalid');
  }
  return {
    kind: 'installed-acceptance-gate',
    operation: 'full-installed-acceptance',
    action: 'verify',
    passed: true,
    exitCode: 0,
    architecture: value.architecture,
    packageSha256: value.candidateInstallerSha256,
    releaseBuildDigest: value.targetReleaseBuildDigest,
    layoutDigest: value.targetPackageLayoutDigest,
    installedState: {
      gatewaySha256: value.targetGatewaySha256,
      ownerSha256: value.targetOwnerSha256,
    },
    sourceCommit: value.sourceCommit,
    sourceTree: value.sourceTree,
    evidence: value,
  };
}

async function evidenceRecords(directory, pinned, rebootRunIds, installedAcceptanceRunId) {
  const records = [];
  for (const arch of ['arm64', 'x64']) {
    let freshClaims;
    for (const [name, operation, action] of SUCCESS_NAMES(arch)) {
      const source = await readFile(resolve(directory, name));
      const claims = validateSuccess(JSON.parse(source), arch, operation, action);
      if (operation === 'fresh') freshClaims = claims;
      records.push({ file: name, sha256: hash(source), claims });
    }
    if (freshClaims === undefined) throw new Error(`${arch} fresh evidence is missing`);
    const migrationName = MIGRATION_NAME(arch);
    const migrationSource = await readFile(resolve(directory, migrationName));
    records.push({
      file: migrationName,
      sha256: hash(migrationSource),
      claims: validateLocalMigration(migrationSource, arch, freshClaims),
    });
    const terminalFaultName = TERMINAL_FAULT_NAME(arch);
    const terminalFaultSource = await readFile(resolve(directory, terminalFaultName));
    const terminalFault = validateTerminalFaultDescriptor(terminalFaultSource, arch, freshClaims);
    records.push({
      file: terminalFaultName,
      sha256: hash(terminalFaultSource),
      claims: {
        kind: 'nonpromotable-fault-descriptor',
        architecture: arch,
        operation: 'reboot-pending-delete',
        action: 'fault',
        passed: true,
        exitCode: 0,
        packageSha256: terminalFault.sha256,
        releaseBuildDigest: freshClaims.releaseBuildDigest,
        layoutDigest: freshClaims.layoutDigest,
        sourceCommit: terminalFault.sourceCommit,
        sourceTree: terminalFault.sourceTree,
        classification: terminalFault.classification,
      },
    });
    const rebootName = REBOOT_NAME(arch);
    const rebootSource = await readFile(resolve(directory, rebootName));
    records.push({
      file: rebootName,
      sha256: hash(rebootSource),
      claims: validateRebootEvidence(
        rebootSource,
        arch,
        terminalFault,
        freshClaims,
        pinned,
        rebootRunIds[arch],
      ),
    });
    validateArchitectureBinding(records, arch);
  }
  const installedSource = await readFile(resolve(directory, INSTALLED_ACCEPTANCE_NAME));
  const installedClaims = validateInstalledAcceptanceGate(
    installedSource,
    installedAcceptanceRunId,
  );
  const x64Fresh = records.find(
    ({ claims }) => claims.architecture === 'x64' && claims.kind === 'success',
  )?.claims;
  if (
    x64Fresh === undefined ||
    installedClaims.releaseBuildDigest !== x64Fresh.releaseBuildDigest ||
    installedClaims.layoutDigest !== x64Fresh.layoutDigest ||
    installedClaims.installedState.gatewaySha256 !== x64Fresh.installedState.gatewaySha256 ||
    installedClaims.installedState.ownerSha256 !== x64Fresh.installedState.ownerSha256
  ) {
    throw new Error('Installed acceptance target differs from the promoted x64 product');
  }
  records.push({
    file: INSTALLED_ACCEPTANCE_NAME,
    sha256: hash(installedSource),
    claims: installedClaims,
  });
  return records.sort((left, right) => left.file.localeCompare(right.file));
}

export async function createWindowsPromotionEvidence({
  directory,
  output,
  repository,
  workflowRunId,
  privateKeyPkcs8Base64,
  publicKeyPath,
  updatePublicKeyPath,
  rebootRunIds,
  installedAcceptanceRunId,
}) {
  if (
    !GENERATION.test(workflowRunId) ||
    !GENERATION.test(installedAcceptanceRunId) ||
    !GENERATION.test(rebootRunIds?.x64 ?? '') ||
    !GENERATION.test(rebootRunIds?.arm64 ?? '') ||
    !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(repository)
  )
    throw new Error('Promotion workflow identity is invalid');
  const pinned = Buffer.from((await readFile(publicKeyPath, 'utf8')).trim(), 'hex');
  const updatePinned = Buffer.from((await readFile(updatePublicKeyPath, 'utf8')).trim(), 'hex');
  if (pinned.length !== 65 || pinned[0] !== 4) throw new Error('Pinned promotion key is invalid');
  if (updatePinned.length !== 65 || updatePinned[0] !== 4 || updatePinned.equals(pinned))
    throw new Error('Promotion and updater public keys must be distinct repository pins');
  const privateKey = createPrivateKey({
    key: Buffer.from(privateKeyPkcs8Base64, 'base64'),
    format: 'der',
    type: 'pkcs8',
  });
  if (!publicSec1(privateKey).equals(pinned))
    throw new Error('Protected promotion key does not match the repository promotion-key pin');
  const records = await evidenceRecords(directory, pinned, rebootRunIds, installedAcceptanceRunId);
  const sourceCommit = records[0].claims.sourceCommit;
  const sourceTree = records[0].claims.sourceTree;
  if (
    records.some(
      ({ claims }) => claims.sourceCommit !== sourceCommit || claims.sourceTree !== sourceTree,
    )
  )
    throw new Error('Lifecycle evidence source identities disagree');
  const keyId = hash(pinned);
  const payload = {
    schemaVersion: 4,
    promotionClass: 'protected-release-acceptance',
    releasePolicy: {
      version: '0.0.69',
      mode: 'fresh-trust-root',
      trustRootVersion: '0.0.69',
      localMigration: {
        sourceVersion: '0.0.67',
        provenance: 'local-non-public',
        mode: 'local-uninstall-preserve-fresh',
      },
    },
    repository,
    workflowRunId,
    sourceCommit,
    sourceTree,
    promotionKeySha256: keyId,
    rebootRunIds,
    installedAcceptanceRunId,
    records,
  };
  const signed = Buffer.concat([DOMAIN, Buffer.from(canonicalJson(payload))]);
  const signature = signBytes('sha256', signed, {
    key: privateKey,
    dsaEncoding: 'ieee-p1363',
  }).toString('base64');
  const envelope = {
    payload,
    signature: { scheme: 'p256-sha256-p1363-v1', keyId, value: signature },
  };
  await writeFile(output, `${canonicalJson(envelope)}\n`, { encoding: 'utf8', mode: 0o600 });
  return envelope;
}

export async function verifyWindowsPromotionEvidence({
  path,
  directory,
  repository,
  workflowRunId,
  publicKeyPath,
  rebootRunIds,
  installedAcceptanceRunId,
}) {
  const source = await readFile(path, 'utf8');
  const envelope = JSON.parse(source);
  exactObject(envelope, ['payload', 'signature'], 'promotion evidence');
  exactObject(
    envelope.payload,
    [
      'schemaVersion',
      'promotionClass',
      'releasePolicy',
      'repository',
      'workflowRunId',
      'sourceCommit',
      'sourceTree',
      'promotionKeySha256',
      'rebootRunIds',
      'installedAcceptanceRunId',
      'records',
    ],
    'promotion payload',
  );
  exactObject(envelope.signature, ['scheme', 'keyId', 'value'], 'promotion signature');
  const pinned = Buffer.from((await readFile(publicKeyPath, 'utf8')).trim(), 'hex');
  const keyId = hash(pinned);
  if (
    envelope.payload.schemaVersion !== 4 ||
    canonicalJson(envelope.payload.releasePolicy) !==
      canonicalJson({
        version: '0.0.69',
        mode: 'fresh-trust-root',
        trustRootVersion: '0.0.69',
        localMigration: {
          sourceVersion: '0.0.67',
          provenance: 'local-non-public',
          mode: 'local-uninstall-preserve-fresh',
        },
      }) ||
    envelope.signature.scheme !== 'p256-sha256-p1363-v1' ||
    envelope.signature.keyId !== keyId ||
    envelope.payload.promotionKeySha256 !== keyId ||
    envelope.payload.promotionClass !== 'protected-release-acceptance' ||
    envelope.payload.repository !== repository ||
    envelope.payload.workflowRunId !== workflowRunId ||
    (installedAcceptanceRunId !== undefined &&
      envelope.payload.installedAcceptanceRunId !== installedAcceptanceRunId)
  )
    throw new Error('Promotion signature identity is invalid');
  const signedRebootRunIds = envelope.payload.rebootRunIds;
  const signedInstalledAcceptanceRunId = envelope.payload.installedAcceptanceRunId;
  if (
    !GENERATION.test(signedInstalledAcceptanceRunId ?? '') ||
    !GENERATION.test(signedRebootRunIds?.x64 ?? '') ||
    !GENERATION.test(signedRebootRunIds?.arm64 ?? '') ||
    (rebootRunIds !== undefined &&
      canonicalJson(rebootRunIds) !== canonicalJson(signedRebootRunIds))
  )
    throw new Error('Reboot workflow identity is invalid');
  const records = await evidenceRecords(
    directory,
    pinned,
    signedRebootRunIds,
    signedInstalledAcceptanceRunId,
  );
  if (canonicalJson(records) !== canonicalJson(envelope.payload.records))
    throw new Error('Promotion evidence inventory or signed claims changed');
  const spkiPrefix = Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex');
  const publicKey = createPublicKey({
    key: Buffer.concat([spkiPrefix, pinned]),
    format: 'der',
    type: 'spki',
  });
  const signed = Buffer.concat([DOMAIN, Buffer.from(canonicalJson(envelope.payload))]);
  const signature = Buffer.from(envelope.signature.value, 'base64');
  if (
    signature.length !== 64 ||
    !verifyBytes('sha256', signed, { key: publicKey, dsaEncoding: 'ieee-p1363' }, signature)
  )
    throw new Error('Promotion lifecycle signature is invalid');
  return envelope;
}

function option(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const rebootX64RunId = option('--reboot-x64-run-id');
  const rebootArm64RunId = option('--reboot-arm64-run-id');
  const installedAcceptanceRunId = option('--installed-acceptance-run-id');
  const common = {
    directory: resolve(option('--directory')),
    repository: option('--repository'),
    workflowRunId: option('--run-id'),
    publicKeyPath: resolve(option('--public-key')),
    installedAcceptanceRunId,
    ...(rebootX64RunId === undefined && rebootArm64RunId === undefined
      ? {}
      : { rebootRunIds: { x64: rebootX64RunId, arm64: rebootArm64RunId } }),
  };
  if (process.argv.includes('--create')) {
    await createWindowsPromotionEvidence({
      ...common,
      output: resolve(option('--output')),
      privateKeyPkcs8Base64:
        process.env.TALKING_QUILL_WINDOWS_PROMOTION_SIGNING_KEY_PKCS8_BASE64 ?? '',
      updatePublicKeyPath: resolve(option('--update-public-key')),
    });
  } else {
    await verifyWindowsPromotionEvidence({ ...common, path: resolve(option('--evidence')) });
  }
}
