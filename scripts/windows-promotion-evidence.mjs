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
const DOMAIN = Buffer.from('TalkingQuill/windows-promotion-lifecycle-evidence/v2\0');
const SUCCESS_NAMES = (arch) => [
  [`windows-installer-success-fresh-${arch}.json`, 'fresh', 'install'],
  [`windows-installer-success-${arch}.json`, 'repair', 'repair'],
];
const FAULT_NAME = (arch) => `windows-installer-fault-recovery-${arch}.json`;

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

const TERMINAL_FAULT_PHASES = [
  'failure-action-restart',
  'post-delete-pre-image-removal',
  'post-final-deletion-ownership',
  'post-final-launcher-ownership',
  'post-final-launcher-posix-delete',
  'post-journal-removal',
  'post-machine-relaunch-owner-clear',
  'post-owner-clear-posix-cleanup',
  'post-maintenance-deletion-ownership',
  'post-maintenance-posix-delete',
  'post-record-pre-start',
  'post-root-tombstone-rename',
  'post-service-pre-record',
  'post-tombstone-content-removal',
  'post-tombstone-marker-removal',
  'post-tombstone-record-removal',
  'post-tombstone-removal',
  'post-uninstall-unregister',
  'pre-CreateService',
  'pre-machine-relaunch-owner-clear',
  'pre-maintenance-deletion-ownership',
  'reboot-pending-delete',
  'service-stopped-pre-DeleteService',
].sort();

function validateTerminalCleanup(value, arch) {
  exactObject(
    value,
    ['acceptanceSetupSha256', 'inspected', 'residue', 'registry', 'scenarios'],
    `${arch} terminal cleanup evidence`,
  );
  const phases = value.scenarios?.map(({ phase }) => phase);
  if (
    value.inspected !== true ||
    !/^[0-9a-f]{64}$/u.test(value.acceptanceSetupSha256) ||
    !Array.isArray(value.scenarios) ||
    value.scenarios.some(
      (scenario) =>
        Object.keys(scenario).sort().join(',') !==
          'acceptanceSetupSha256,installedUninstallerSha256,isolated,phase,recovered' ||
        scenario.acceptanceSetupSha256 !== value.acceptanceSetupSha256 ||
        scenario.installedUninstallerSha256 !== value.acceptanceSetupSha256 ||
        scenario.isolated !== true ||
        scenario.recovered !== true,
    ) ||
    new Set(phases).size !== phases.length ||
    [...phases].sort().join(',') !== TERMINAL_FAULT_PHASES.join(',') ||
    !Array.isArray(value.residue) ||
    value.residue.length !== 0 ||
    !Array.isArray(value.registry) ||
    value.registry.length !== 0
  )
    throw new Error(`${arch} terminal cleanup evidence is invalid`);
  return {
    acceptanceSetupSha256: value.acceptanceSetupSha256,
    inspected: true,
    scenarios: value.scenarios,
    residue: [],
    registry: [],
  };
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

function validateFault(value, arch) {
  exactObject(
    value,
    [
      'schemaVersion',
      'architecture',
      'sourceCommit',
      'sourceTree',
      'productionUpdateInterrupted',
      'candidateSha256',
      'releaseBuildDigest',
      'faults',
      'terminalCleanup',
      'passed',
    ],
    `${arch} fault evidence`,
  );
  if (
    value.schemaVersion !== 1 ||
    value.architecture !== arch ||
    value.passed !== true ||
    value.productionUpdateInterrupted !== true ||
    !SOURCE.test(value.sourceCommit) ||
    !SOURCE.test(value.sourceTree) ||
    !SHA256.test(value.candidateSha256) ||
    !SHA256.test(value.releaseBuildDigest) ||
    !Array.isArray(value.faults) ||
    value.faults.length !== 10
  )
    throw new Error(`${arch} fault evidence is invalid`);
  const terminalCleanup = validateTerminalCleanup(value.terminalCleanup, arch);
  for (const fault of value.faults) {
    exactObject(
      fault,
      [
        'fault',
        'crashExitCode',
        'recoveryExitCode',
        'inspectedBeforeRepair',
        'productionRecoveryInterrupted',
        'recoveredReleaseBuildDigest',
        'appPathExact',
        'quietUninstallExact',
        'legacyServiceAbsent',
        'legacyTaskAbsent',
      ],
      `${arch} fault phase result`,
    );
    if (
      typeof fault.fault !== 'string' ||
      fault.fault.length === 0 ||
      fault.crashExitCode !== 197 ||
      fault.recoveryExitCode !== 78 ||
      fault.inspectedBeforeRepair !== true ||
      fault.productionRecoveryInterrupted !== true ||
      fault.recoveredReleaseBuildDigest !== value.releaseBuildDigest ||
      fault.appPathExact !== true ||
      fault.quietUninstallExact !== true ||
      fault.legacyServiceAbsent !== true ||
      fault.legacyTaskAbsent !== true
    )
      throw new Error(`${arch} fault phase result is invalid`);
  }
  const expectedFaults = [
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
  ];
  const actualFaults = value.faults
    .map(({ fault }) => fault.replace(/^.*repair-|\.exe$/gu, ''))
    .sort();
  if (actualFaults.join(',') !== expectedFaults.sort().join(','))
    throw new Error(`${arch} fault phase inventory is invalid`);
  return {
    kind: 'fault',
    architecture: arch,
    operation: 'fault-recovery',
    action: 'fault',
    passed: value.passed,
    exitCode: 0,
    packageSha256: value.candidateSha256,
    releaseBuildDigest: value.releaseBuildDigest,
    layoutDigest: value.releaseBuildDigest,
    sourceCommit: value.sourceCommit,
    sourceTree: value.sourceTree,
    processTokenImageKernelObservations: [],
    peerObservation: null,
    installedState: null,
    faultPhase: value.faults.map(({ fault }) => fault),
    faultResult: value.faults,
    terminalCleanup,
    evidence: value,
  };
}

function validateArchitectureBinding(records, arch) {
  const claims = records
    .filter(({ claims: record }) => record.architecture === arch)
    .map(({ claims: record }) => record);
  const fresh = claims.find(({ operation }) => operation === 'fresh');
  const repair = claims.find(({ operation }) => operation === 'repair');
  const fault = claims.find(({ operation }) => operation === 'fault-recovery');
  if (
    fresh === undefined ||
    repair === undefined ||
    fault === undefined ||
    fault.packageSha256 !== repair.packageSha256 ||
    fault.releaseBuildDigest !== repair.releaseBuildDigest ||
    fresh.releaseBuildDigest !== repair.releaseBuildDigest ||
    fresh.sourceCommit !== repair.sourceCommit ||
    fresh.sourceTree !== repair.sourceTree ||
    fault.sourceCommit !== repair.sourceCommit ||
    fault.sourceTree !== repair.sourceTree ||
    fresh.installedState.gatewaySha256 !== repair.installedState.gatewaySha256 ||
    fresh.installedState.ownerSha256 !== repair.installedState.ownerSha256
  )
    throw new Error(`${arch} promotion evidence generation binding is invalid`);
}

async function evidenceRecords(directory) {
  const records = [];
  for (const arch of ['arm64', 'x64']) {
    for (const [name, operation, action] of SUCCESS_NAMES(arch)) {
      const source = await readFile(resolve(directory, name));
      records.push({
        file: name,
        sha256: hash(source),
        claims: validateSuccess(JSON.parse(source), arch, operation, action),
      });
    }
    const name = FAULT_NAME(arch);
    const source = await readFile(resolve(directory, name));
    records.push({
      file: name,
      sha256: hash(source),
      claims: validateFault(JSON.parse(source), arch),
    });
    validateArchitectureBinding(records, arch);
  }
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
}) {
  if (!GENERATION.test(workflowRunId) || !/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/u.test(repository))
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
  const records = await evidenceRecords(directory);
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
    schemaVersion: 2,
    promotionClass: 'protected-release-acceptance',
    repository,
    workflowRunId,
    sourceCommit,
    sourceTree,
    promotionKeySha256: keyId,
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
}) {
  const source = await readFile(path, 'utf8');
  const envelope = JSON.parse(source);
  exactObject(envelope, ['payload', 'signature'], 'promotion evidence');
  exactObject(
    envelope.payload,
    [
      'schemaVersion',
      'promotionClass',
      'repository',
      'workflowRunId',
      'sourceCommit',
      'sourceTree',
      'promotionKeySha256',
      'records',
    ],
    'promotion payload',
  );
  exactObject(envelope.signature, ['scheme', 'keyId', 'value'], 'promotion signature');
  const pinned = Buffer.from((await readFile(publicKeyPath, 'utf8')).trim(), 'hex');
  const keyId = hash(pinned);
  if (
    envelope.payload.schemaVersion !== 2 ||
    envelope.signature.scheme !== 'p256-sha256-p1363-v1' ||
    envelope.signature.keyId !== keyId ||
    envelope.payload.promotionKeySha256 !== keyId ||
    envelope.payload.promotionClass !== 'protected-release-acceptance' ||
    envelope.payload.repository !== repository ||
    envelope.payload.workflowRunId !== workflowRunId
  )
    throw new Error('Promotion signature identity is invalid');
  const records = await evidenceRecords(directory);
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
  const common = {
    directory: resolve(option('--directory')),
    repository: option('--repository'),
    workflowRunId: option('--run-id'),
    publicKeyPath: resolve(option('--public-key')),
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
