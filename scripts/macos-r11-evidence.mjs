import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';

export const MACOS_R11_SCHEMA_VERSION = 1;
export const MACOS_R11_KIND = 'macos-r11-installed-lifecycle';
export const MACOS_R11_CHECKPOINTS = Object.freeze([
  'dmg-install-exact-app',
  'local-install-anyway',
  'stable-local-identity',
  'smappservice-register-launch-unregister',
  'keychain-owner-allow',
  'keychain-gateway-allow',
  'keychain-electron-deny',
  'tcc-grant-capture',
  'tcc-revoke-fail-closed',
  'tcc-regrant-recovery',
  'baseline-tcc-grant-capture',
  'physical-shortcut-held',
  'held-key-finalizer-postponed',
  'physical-shortcut-replay',
  'physical-option-command-replay',
  'physical-paste',
  'zip-update-exact-app',
  'candidate-tcc-post-update-recovery',
  'controlled-rollback',
  'owner-crash-recovery',
  'drag-to-trash-cleanup',
  'controlled-uninstall-cleanup',
  'persisted-identity-continuity',
]);

const HEX_256 = /^[0-9a-f]{64}$/u;
const SHA = /^[0-9a-f]{40}$/u;
const DECIMAL = /^(?:0|[1-9]\d*)$/u;
const TAG = /^v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)$/u;
const ARCHES = new Set(['x64', 'arm64']);
const METHODS = new Set(['automated', 'staffed-manual']);

export function sha256File(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}

export function canonicalJson(value) {
  if (value === null || typeof value !== 'object') return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`;
}

export function sealMacosR11Evidence(body) {
  const unsealed = structuredClone(body);
  delete unsealed.evidenceSha256;
  return {
    ...unsealed,
    evidenceSha256: createHash('sha256').update(canonicalJson(unsealed)).digest('hex'),
  };
}

export function validateMacosR11Evidence(value, expected = {}) {
  if (!HEX_256.test(expected.operatorPublicKeySha256 ?? '')) {
    fail('Trusted operator public key pin is required');
  }
  if (!HEX_256.test(expected.sessionBindingSha256 ?? '')) {
    fail('Expected lifecycle session binding is required');
  }
  if (!HEX_256.test(expected.evidenceSha256 ?? '')) {
    fail('Externally trusted evidence digest is required');
  }
  object(value, 'Evidence');
  exactKeys(
    value,
    [
      'schemaVersion',
      'kind',
      'platform',
      'arch',
      'sourceCommit',
      'candidateTag',
      'sessionBindingSha256',
      'releaseRun',
      'lifecycleRun',
      'runner',
      'signing',
      'artifacts',
      'predecessor',
      'installed',
      'checkpoints',
      'result',
      'evidenceSha256',
    ],
    'Evidence',
  );
  if (value.schemaVersion !== MACOS_R11_SCHEMA_VERSION || value.kind !== MACOS_R11_KIND) {
    fail('Unsupported macOS R11 evidence schema');
  }
  if (value.platform !== 'mac' || !ARCHES.has(value.arch)) fail('Invalid macOS architecture');
  if (!SHA.test(value.sourceCommit) || !TAG.test(value.candidateTag))
    fail('Invalid release identity');
  if (
    !HEX_256.test(value.sessionBindingSha256) ||
    value.sessionBindingSha256 !== expected.sessionBindingSha256
  ) {
    fail('Lifecycle session binding mismatch');
  }
  if (expected.arch !== undefined && value.arch !== expected.arch)
    fail('Evidence architecture mismatch');
  if (expected.sourceCommit !== undefined && value.sourceCommit !== expected.sourceCommit) {
    fail('Evidence source commit mismatch');
  }
  if (expected.candidateTag !== undefined && value.candidateTag !== expected.candidateTag) {
    fail('Evidence release tag mismatch');
  }

  runIdentity(value.releaseRun, 'Release run');
  runIdentity(value.lifecycleRun, 'Lifecycle run');
  object(value.runner, 'Runner');
  exactKeys(value.runner, ['os', 'arch', 'name'], 'Runner');
  if (value.runner.os !== 'macOS' || typeof value.runner.name !== 'string' || !value.runner.name) {
    fail('Evidence was not produced on a named macOS runner');
  }
  const nativeRunnerArch = value.arch === 'x64' ? 'X64' : 'ARM64';
  if (value.runner.arch !== nativeRunnerArch)
    fail('Evidence runner is not native for its artifact');

  validateSigning(value.signing);
  validateArtifacts(value.artifacts, value.arch, expected);
  validatePredecessor(value.predecessor, value.arch, value.artifacts.baselineZip);
  validateInstalled(value.installed);
  validateCheckpoints(
    value.checkpoints,
    value.signing.mode,
    value.sessionBindingSha256,
    expected.operatorPublicKeySha256,
  );
  if (value.result !== 'passed') fail('macOS lifecycle evidence did not pass');

  const sealed = structuredClone(value);
  delete sealed.evidenceSha256;
  const digest = createHash('sha256').update(canonicalJson(sealed)).digest('hex');
  if (
    !HEX_256.test(value.evidenceSha256) ||
    value.evidenceSha256 !== digest ||
    value.evidenceSha256 !== expected.evidenceSha256
  ) {
    fail('macOS lifecycle evidence seal mismatch');
  }
  return value;
}

export function writeMacosR11Evidence(path, body, expected = {}) {
  const evidence = sealMacosR11Evidence(body);
  validateMacosR11Evidence(evidence, {
    ...expected,
    evidenceSha256: evidence.evidenceSha256,
  });
  writeFileSync(path, `${JSON.stringify(evidence, null, 2)}\n`, { flag: 'wx', mode: 0o600 });
  return evidence;
}

function validateSigning(value) {
  object(value, 'Signing');
  exactKeys(
    value,
    [
      'mode',
      'candidateRequirement',
      'baselineRequirement',
      'candidateLeafSha256',
      'baselineLeafSha256',
      'candidateCdHash',
      'baselineCdHash',
    ],
    'Signing',
  );
  if (value.mode !== 'self-signed' && value.mode !== 'adhoc') fail('Invalid local signing mode');
  for (const name of ['candidateRequirement', 'baselineRequirement']) {
    if (typeof value[name] !== 'string' || value[name].length < 8) fail(`Invalid ${name}`);
  }
  for (const name of ['candidateCdHash', 'baselineCdHash']) {
    if (!/^[0-9a-f]{40,64}$/u.test(value[name])) fail(`Invalid ${name}`);
  }
  if (value.mode === 'self-signed') {
    if (
      !HEX_256.test(value.candidateLeafSha256) ||
      value.candidateLeafSha256 !== value.baselineLeafSha256
    ) {
      fail('Self-signed lifecycle did not preserve the exact local certificate');
    }
    if (value.candidateRequirement !== value.baselineRequirement) {
      fail('Self-signed lifecycle did not preserve the designated requirement');
    }
  } else if (value.candidateLeafSha256 !== null || value.baselineLeafSha256 !== null) {
    fail('Ad-hoc lifecycle evidence must not claim a certificate leaf');
  }
}

function validateArtifacts(value, arch, expected) {
  object(value, 'Artifacts');
  exactKeys(value, ['candidateDmg', 'candidateZip', 'baselineZip'], 'Artifacts');
  for (const [role, artifact] of Object.entries(value)) {
    object(artifact, `Artifact ${role}`);
    exactKeys(artifact, ['name', 'bytes', 'sha256Before', 'sha256After'], `Artifact ${role}`);
    if (
      typeof artifact.name !== 'string' ||
      artifact.name.includes('/') ||
      artifact.name.includes('\\')
    ) {
      fail(`Invalid artifact name: ${role}`);
    }
    const suffix = role === 'candidateDmg' ? `.dmg` : `.zip`;
    if (!artifact.name.endsWith(`-mac-${arch}${suffix}`)) fail(`Artifact target mismatch: ${role}`);
    if (!Number.isSafeInteger(artifact.bytes) || artifact.bytes < 1)
      fail(`Invalid artifact size: ${role}`);
    if (!HEX_256.test(artifact.sha256Before) || artifact.sha256Before !== artifact.sha256After) {
      fail(`Artifact bytes changed during lifecycle: ${role}`);
    }
  }
  if (value.candidateDmg.sha256Before === value.candidateZip.sha256Before) {
    fail('DMG and ZIP evidence were not independently hashed');
  }
  const candidateVersion = artifactVersion(value.candidateZip.name, arch);
  const dmgVersion = artifactVersion(value.candidateDmg.name, arch);
  const baselineVersion = artifactVersion(value.baselineZip.name, arch);
  if (
    candidateVersion.join('.') !== dmgVersion.join('.') ||
    compareVersion(candidateVersion, baselineVersion) <= 0
  ) {
    fail('Accepted macOS baseline is not an older coherent predecessor');
  }
  for (const [role, digest] of Object.entries(expected.artifactSha256 ?? {})) {
    if (value[role]?.sha256Before !== digest) fail(`Expected artifact hash mismatch: ${role}`);
  }
}

function artifactVersion(name, arch) {
  const match = new RegExp(
    `^Talking-Quill-(\\d+)\\.(\\d+)\\.(\\d+)-mac-${arch}\\.(?:dmg|zip)$`,
    'u',
  ).exec(name);
  if (!match) fail('Artifact filename has no canonical semantic version');
  return match.slice(1).map(Number);
}
function compareVersion(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

function validatePredecessor(value, arch, baselineZip) {
  object(value, 'Predecessor');
  exactKeys(
    value,
    [
      'platform',
      'architecture',
      'version',
      'runId',
      'runAttempt',
      'headSha',
      'artifactName',
      'releaseBuildDigest',
      'gatewaySha256',
      'ownerSha256',
      'artifactSha256',
    ],
    'Predecessor',
  );
  if (
    value.platform !== 'mac' ||
    value.architecture !== arch ||
    !/^\d+\.\d+\.\d+$/u.test(value.version ?? '') ||
    !DECIMAL.test(value.runId ?? '') ||
    value.runId === '0' ||
    !DECIMAL.test(value.runAttempt ?? '') ||
    value.runAttempt === '0' ||
    !SHA.test(value.headSha ?? '') ||
    value.artifactName !== baselineZip.name ||
    !HEX_256.test(value.releaseBuildDigest ?? '') ||
    !HEX_256.test(value.gatewaySha256 ?? '') ||
    !HEX_256.test(value.ownerSha256 ?? '') ||
    value.artifactSha256 !== baselineZip.sha256Before ||
    !baselineZip.name.includes(`-${value.version}-mac-${arch}.zip`)
  )
    fail('Tested macOS predecessor identity is invalid or mismatched');
}

function validateInstalled(value) {
  object(value, 'Installed evidence');
  exactKeys(
    value,
    [
      'dmgAppTreeSha256',
      'zipAppTreeSha256',
      'updatedAppTreeSha256',
      'rollbackAppTreeSha256',
      'candidateGatewaySha256',
      'candidateOwnerSha256',
      'baselineGatewaySha256',
      'baselineOwnerSha256',
      'installationIdSha256',
    ],
    'Installed evidence',
  );
  for (const [name, digest] of Object.entries(value)) {
    if (!HEX_256.test(digest)) fail(`Invalid installed digest: ${name}`);
  }
  if (
    value.dmgAppTreeSha256 !== value.zipAppTreeSha256 ||
    value.zipAppTreeSha256 !== value.updatedAppTreeSha256
  ) {
    fail('Candidate DMG, ZIP, and updated application trees differ');
  }
}

function validateCheckpoints(value, signingMode, sessionBindingSha256, expectedOperatorKey) {
  if (!Array.isArray(value) || value.length !== MACOS_R11_CHECKPOINTS.length) {
    fail('Incomplete macOS R11 checkpoint set');
  }
  const expected = new Set(MACOS_R11_CHECKPOINTS);
  const seen = new Set();
  for (const checkpoint of value) {
    object(checkpoint, 'Checkpoint');
    exactKeys(
      checkpoint,
      ['id', 'method', 'challengeSha256', 'observedAt', 'result', 'attestation'],
      'Checkpoint',
    );
    if (!expected.has(checkpoint.id) || seen.has(checkpoint.id))
      fail('Unknown or duplicate checkpoint');
    seen.add(checkpoint.id);
    if (!METHODS.has(checkpoint.method) || !HEX_256.test(checkpoint.challengeSha256)) {
      fail(`Invalid checkpoint semantics: ${checkpoint.id}`);
    }
    if (!Number.isSafeInteger(checkpoint.observedAt) || checkpoint.observedAt < 1) {
      fail(`Invalid checkpoint timestamp: ${checkpoint.id}`);
    }
    if (checkpoint.result !== 'passed') fail(`Checkpoint did not pass: ${checkpoint.id}`);
    if (checkpoint.method === 'staffed-manual') {
      validateAttestation(checkpoint, sessionBindingSha256, expectedOperatorKey);
    } else if (checkpoint.attestation !== null) {
      fail(`Automated checkpoint has an operator attestation: ${checkpoint.id}`);
    }
  }
  for (const id of [
    'local-install-anyway',
    'keychain-owner-allow',
    'tcc-grant-capture',
    'tcc-revoke-fail-closed',
    'tcc-regrant-recovery',
    'baseline-tcc-grant-capture',
    'candidate-tcc-post-update-recovery',
    'persisted-identity-continuity',
    'drag-to-trash-cleanup',
    'controlled-uninstall-cleanup',
    'physical-shortcut-held',
    'physical-shortcut-replay',
    'physical-option-command-replay',
    'physical-paste',
  ]) {
    const checkpoint = value.find((entry) => entry.id === id);
    if (checkpoint.method !== 'staffed-manual')
      fail(`Physical/manual checkpoint was automated: ${id}`);
  }
  if (signingMode === 'adhoc') {
    const recovery = value.find((entry) => entry.id === 'candidate-tcc-post-update-recovery');
    if (recovery.method !== 'staffed-manual')
      fail('Ad-hoc identity requires staffed TCC re-enrollment');
  }
}

function validateAttestation(checkpoint, sessionBindingSha256, expectedOperatorKey) {
  const value = checkpoint.attestation;
  object(value, `Attestation ${checkpoint.id}`);
  exactKeys(
    value,
    ['algorithm', 'publicKeySpkiBase64', 'publicKeySha256', 'payloadBase64', 'signatureBase64'],
    `Attestation ${checkpoint.id}`,
  );
  if (value.algorithm !== 'ed25519') fail(`Invalid attestation algorithm: ${checkpoint.id}`);
  let publicKey;
  let payload;
  let signature;
  try {
    const der = Buffer.from(value.publicKeySpkiBase64, 'base64');
    payload = Buffer.from(value.payloadBase64, 'base64');
    signature = Buffer.from(value.signatureBase64, 'base64');
    if (createHash('sha256').update(der).digest('hex') !== value.publicKeySha256) {
      fail(`Operator public key digest mismatch: ${checkpoint.id}`);
    }
    publicKey = createPublicKey({ key: der, format: 'der', type: 'spki' });
  } catch {
    fail(`Malformed operator attestation: ${checkpoint.id}`);
  }
  if (expectedOperatorKey !== undefined && value.publicKeySha256 !== expectedOperatorKey) {
    fail(`Unexpected operator attestation key: ${checkpoint.id}`);
  }
  let statement;
  try {
    statement = JSON.parse(payload.toString('utf8'));
  } catch {
    fail(`Malformed operator statement: ${checkpoint.id}`);
  }
  object(statement, `Operator statement ${checkpoint.id}`);
  exactKeys(
    statement,
    ['id', 'challengeSha256', 'sessionBindingSha256', 'result', 'observedAt', 'operator', 'host'],
    `Operator statement ${checkpoint.id}`,
  );
  if (
    statement.id !== checkpoint.id ||
    statement.challengeSha256 !== checkpoint.challengeSha256 ||
    statement.sessionBindingSha256 !== sessionBindingSha256 ||
    statement.result !== 'passed' ||
    statement.observedAt !== checkpoint.observedAt ||
    typeof statement.operator !== 'string' ||
    !statement.operator ||
    typeof statement.host !== 'string' ||
    !statement.host ||
    !verify(null, payload, publicKey, signature)
  ) {
    fail(`Invalid operator attestation: ${checkpoint.id}`);
  }
}

function runIdentity(value, label) {
  object(value, label);
  exactKeys(value, ['id', 'attempt'], label);
  if (
    !DECIMAL.test(value.id) ||
    !DECIMAL.test(value.attempt) ||
    value.id === '0' ||
    value.attempt === '0'
  ) {
    fail(`Invalid ${label.toLowerCase()} identity`);
  }
}
function object(value, label) {
  if (value === null || typeof value !== 'object' || Array.isArray(value))
    fail(`${label} must be an object`);
}
function exactKeys(value, keys, label) {
  const actual = Object.keys(value).sort();
  const wanted = [...keys].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted))
    fail(`${label} has unexpected or missing fields`);
}
function fail(message) {
  throw new Error(message);
}
