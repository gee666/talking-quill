import { createHash, createPublicKey, verify } from 'node:crypto';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { ACCEPTANCE_FAULT_PHASES } from './windows-installed-acceptance-schedule.mjs';

const HEX = /^[0-9a-f]{64}$/u;

export function faultEvidenceGenesis(buildId, candidateSha256) {
  if (!HEX.test(buildId ?? '') || !HEX.test(candidateSha256 ?? '')) {
    throw new Error('Fault evidence genesis identity is invalid');
  }
  return sha256(
    Buffer.from(
      canonicalAcceptanceJson({
        purpose: 'talking-quill/installed-acceptance-fault-validation/genesis',
        buildId,
        candidateSha256,
      }),
    ),
  );
}

export function encodeSignedFaultEvidence(payload, signed) {
  validatePayloadShape(payload);
  if (!/^[A-Za-z0-9_-]+$/u.test(signed.signatureBase64url ?? '')) {
    throw new Error('Fault validation signature is invalid');
  }
  return Buffer.from(
    `${canonicalAcceptanceJson({ payload, signatureBase64url: signed.signatureBase64url })}\n`,
  );
}

export function verifyFaultEvidenceChain(records, expectation) {
  if (
    !Array.isArray(records) ||
    records.length !== ACCEPTANCE_FAULT_PHASES.length ||
    !HEX.test(expectation.buildId ?? '') ||
    !HEX.test(expectation.candidateSha256 ?? '')
  ) {
    throw new Error('Fault validation chain inventory is invalid');
  }
  const key = readPublicKey(expectation.publicKeySpkiBase64url);
  let previous = faultEvidenceGenesis(expectation.buildId, expectation.candidateSha256);
  const hashes = [];
  for (let index = 0; index < records.length; index += 1) {
    const record = records[index];
    const bytes = Buffer.isBuffer(record.bytes) ? record.bytes : Buffer.from(record.bytes);
    let envelope;
    try {
      envelope = JSON.parse(bytes.toString('utf8'));
    } catch {
      throw new Error('Fault validation evidence is not JSON');
    }
    if (bytes.toString('utf8') !== `${canonicalAcceptanceJson(envelope)}\n`) {
      throw new Error('Fault validation evidence is not canonical');
    }
    validatePayloadShape(envelope.payload);
    const payload = envelope.payload;
    const artifact = record.artifact;
    if (
      payload.sequence !== index ||
      payload.faultPhase !== ACCEPTANCE_FAULT_PHASES[index] ||
      payload.previousEnvelopeSha256 !== previous ||
      payload.buildId !== expectation.buildId ||
      payload.architecture !== expectation.architecture ||
      payload.sourceCommit !== expectation.sourceCommit ||
      payload.sourceTree !== expectation.sourceTree ||
      payload.candidatePackageSha256 !== expectation.candidateSha256 ||
      payload.candidatePackageLayoutDigest !== expectation.candidateLayoutDigest ||
      payload.faultPackageSha256 !== artifact.installer.sha256 ||
      payload.faultPackageTreeSha256 !== artifact.packageManifest.treeSha256 ||
      payload.failurePointObserved !== payload.faultPhase ||
      payload.failureInjected !== true ||
      payload.recoveryCompleted !== true ||
      payload.zeroResidue !== true ||
      payload.productionStateUnchanged !== true ||
      payload.mixedAuthorityAbsent !== true
    ) {
      throw new Error(
        `Fault validation evidence binding is invalid: ${String(payload.faultPhase)}`,
      );
    }
    const signature = Buffer.from(envelope.signatureBase64url ?? '', 'base64url');
    if (
      signature.length !== 64 ||
      signature.toString('base64url') !== envelope.signatureBase64url ||
      !verify(
        'sha256',
        Buffer.from(canonicalAcceptanceJson(payload)),
        {
          key,
          dsaEncoding: 'ieee-p1363',
        },
        signature,
      )
    ) {
      throw new Error(`Fault validation signature is invalid: ${payload.faultPhase}`);
    }
    previous = sha256(bytes);
    hashes.push(previous);
  }
  if (expectation.chainHeadSha256 !== undefined && previous !== expectation.chainHeadSha256) {
    throw new Error('Fault validation chain head differs from its trusted binding');
  }
  return Object.freeze({ chainHeadSha256: previous, envelopeSha256: Object.freeze(hashes) });
}

function validatePayloadShape(payload) {
  const keys = [
    'architecture',
    'buildId',
    'candidatePackageLayoutDigest',
    'candidatePackageSha256',
    'failureInjected',
    'failurePointObserved',
    'faultExitCode',
    'faultPackageSha256',
    'faultPackageTreeSha256',
    'faultPhase',
    'isolatedStateAfterSha256',
    'isolatedStateBeforeSha256',
    'machineIdentitySha256',
    'mixedAuthorityAbsent',
    'namespaceIdSha256',
    'previousEnvelopeSha256',
    'productionStateAfterSha256',
    'productionStateBeforeSha256',
    'productionStateUnchanged',
    'purpose',
    'recoveryCompleted',
    'recoveryExitCode',
    'schemaVersion',
    'sequence',
    'sessionIdentitySha256',
    'sourceCommit',
    'sourceTree',
    'validatorSha256',
    'zeroResidue',
  ];
  if (
    payload === null ||
    typeof payload !== 'object' ||
    Object.keys(payload).sort().join(',') !== keys.sort().join(',') ||
    payload.schemaVersion !== 1 ||
    payload.purpose !== 'talking-quill/installed-acceptance-fault-validation' ||
    !Number.isSafeInteger(payload.sequence) ||
    !Number.isSafeInteger(payload.faultExitCode) ||
    !Number.isSafeInteger(payload.recoveryExitCode) ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceTree ?? '') ||
    !Object.entries(payload)
      .filter(([name]) => name.endsWith('Sha256') || name === 'buildId')
      .every(([, value]) => HEX.test(value))
  ) {
    throw new Error('Fault validation evidence schema is invalid');
  }
}

function readPublicKey(encoded) {
  if (!/^[A-Za-z0-9_-]+$/u.test(encoded ?? '')) {
    throw new Error('Fault validation public key is invalid');
  }
  const bytes = Buffer.from(encoded, 'base64url');
  if (bytes.toString('base64url') !== encoded) {
    throw new Error('Fault validation public key is invalid');
  }
  const key = createPublicKey({ key: bytes, format: 'der', type: 'spki' });
  if (key.asymmetricKeyType !== 'ec' || key.asymmetricKeyDetails?.namedCurve !== 'prime256v1') {
    throw new Error('Fault validation public key must be P-256');
  }
  return key;
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
