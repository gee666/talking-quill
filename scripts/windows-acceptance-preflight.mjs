import { createPublicKey, verify } from 'node:crypto';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';

const HEX_32 = /^[0-9a-f]{64}$/u;
const BASE64URL = /^[A-Za-z0-9_-]+$/u;
const MAX_ENVELOPE_BYTES = 16 * 1024;

export async function verifyAcceptancePreflight(input) {
  const { plan, sequence, nowMs } = input;
  const manifest = decodeCanonicalEnvelope(
    plan.acceptance.buildManifest,
    'acceptance build manifest',
  );
  const manifestKey = readP256PublicKey(
    plan.acceptance.manifestPublicKeySpkiBase64url,
    'acceptance manifest public key',
  );
  verifyEnvelopeSignature(manifest, manifestKey, 'Acceptance build manifest');
  const payload = manifest.payload;
  const candidate = plan.artifacts.candidate;
  const gateway = role(candidate.metadata, 'gateway');
  const owner = role(candidate.metadata, 'owner');
  const acceptancePayload = candidate.releaseIdentity?.acceptancePayload;
  if (
    payload?.version !== 1 ||
    payload.purpose !== 'talking-quill/installed-acceptance-build' ||
    payload.sourceRevision !== plan.acceptance.sourceRevision ||
    payload.buildId !== sequence.buildId ||
    payload.architecture !== plan.architecture ||
    payload.packageVersion !== candidate.metadata.version ||
    payload.releaseBuildDigest !== candidate.metadata.releaseBuildDigest ||
    payload.packageLayoutDigest !== candidate.metadata.packageLayoutDigest ||
    payload.ownerManifestSha256 !== candidate.metadataIdentity.sha256 ||
    payload.electronSha256 !== candidate.electron.sha256 ||
    payload.appAsarSha256 !== candidate.appAsar.sha256 ||
    payload.gatewaySha256 !== gateway.sha256 ||
    payload.ownerSha256 !== owner.sha256 ||
    acceptancePayload?.schemaVersion !== 1 ||
    acceptancePayload?.installerSha256 !== candidate.installer.sha256 ||
    acceptancePayload?.electronSha256 !== candidate.electron.sha256 ||
    acceptancePayload?.appAsarSha256 !== candidate.appAsar.sha256 ||
    acceptancePayload?.buildManifestSha256 !== plan.acceptance.buildManifestIdentity.sha256 ||
    !Number.isSafeInteger(payload.validFromMs) ||
    !Number.isSafeInteger(payload.validUntilMs) ||
    nowMs < payload.validFromMs ||
    nowMs > payload.validUntilMs ||
    sequence.runWindow.notBeforeMs < payload.validFromMs ||
    sequence.runWindow.expiresAtMs > payload.validUntilMs
  ) {
    throw new Error('Acceptance build manifest binding is invalid');
  }
  const requestKey = readP256PublicKey(
    payload.requestPublicKeySpkiBase64url,
    'acceptance request public key',
  );
  for (const request of sequence.requests) {
    const envelope = decodeCanonicalEnvelope(request.encoded, 'acceptance run request');
    verifyEnvelopeSignature(envelope, requestKey, `Acceptance request ${request.invocationId}`);
    if (canonicalAcceptanceJson(envelope.payload) !== canonicalAcceptanceJson(request.payload)) {
      throw new Error(
        `Acceptance request payload changed during preflight: ${request.invocationId}`,
      );
    }
  }
  let reservedNonceCount = 0;
  if (input.reserveNonces === true) {
    if (typeof input.reserveReplayNonces !== 'function') {
      throw new Error('Acceptance replay reservation is unavailable');
    }
    const reservation = await input.reserveReplayNonces(sequence.requests);
    if (reservation?.reservedCount !== sequence.requests.length) {
      throw new Error('Acceptance replay reservation count is invalid');
    }
    reservedNonceCount = reservation.reservedCount;
  }
  return Object.freeze({
    buildId: sequence.buildId,
    manifestBuildId: payload.buildId,
    requestCount: sequence.requests.length,
    signaturesVerified: sequence.requests.length + 1,
    reservedNonceCount,
  });
}

function decodeCanonicalEnvelope(encoded, label) {
  if (typeof encoded !== 'string' || !BASE64URL.test(encoded)) {
    throw new Error(`Encoded ${label} is invalid`);
  }
  const bytes = Buffer.from(encoded, 'base64url');
  if (
    bytes.length === 0 ||
    bytes.length > MAX_ENVELOPE_BYTES ||
    bytes.toString('base64url') !== encoded
  ) {
    throw new Error(`Encoded ${label} is invalid`);
  }
  let envelope;
  try {
    envelope = JSON.parse(bytes.toString('utf8'));
  } catch {
    throw new Error(`Encoded ${label} is not JSON`);
  }
  if (
    envelope === null ||
    typeof envelope !== 'object' ||
    envelope.payload === null ||
    typeof envelope.payload !== 'object' ||
    typeof envelope.signatureBase64url !== 'string' ||
    Buffer.from(canonicalAcceptanceJson(envelope)).toString('base64url') !== encoded
  ) {
    throw new Error(`Encoded ${label} is not canonical`);
  }
  return envelope;
}

function readP256PublicKey(encoded, label) {
  if (typeof encoded !== 'string' || !BASE64URL.test(encoded) || encoded.length > 256) {
    throw new Error(`${label} is invalid`);
  }
  const der = Buffer.from(encoded, 'base64url');
  if (der.toString('base64url') !== encoded) throw new Error(`${label} is invalid`);
  let key;
  try {
    key = createPublicKey({ key: der, format: 'der', type: 'spki' });
  } catch {
    throw new Error(`${label} is invalid`);
  }
  if (
    key.asymmetricKeyType !== 'ec' ||
    key.asymmetricKeyDetails?.namedCurve !== 'prime256v1' ||
    !Buffer.from(key.export({ format: 'der', type: 'spki' })).equals(der)
  ) {
    throw new Error(`${label} must be a canonical P-256 public key`);
  }
  return key;
}

function verifyEnvelopeSignature(envelope, key, label) {
  const signature = Buffer.from(envelope.signatureBase64url, 'base64url');
  if (
    signature.length !== 64 ||
    signature.toString('base64url') !== envelope.signatureBase64url ||
    !verify(
      'sha256',
      Buffer.from(canonicalAcceptanceJson(envelope.payload)),
      { key, dsaEncoding: 'ieee-p1363' },
      signature,
    )
  ) {
    throw new Error(`${label} signature is invalid`);
  }
}

function role(metadata, name) {
  const value = metadata.roles?.find((entry) => entry.role === name);
  if (!value || !HEX_32.test(value.sha256 ?? '')) {
    throw new Error(`Acceptance candidate ${name} binding is missing`);
  }
  return value;
}
