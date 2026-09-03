import { createPublicKey, verify } from 'node:crypto';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export function verifyWindowsAcceptanceAuthorization(input) {
  const encoded = input.authorizationBase64url;
  if (typeof encoded !== 'string' || !/^[A-Za-z0-9_-]+$/u.test(encoded)) {
    throw new Error('Acceptance bundle authorization is invalid');
  }
  const bytes = Buffer.from(encoded, 'base64url');
  if (bytes.length === 0 || bytes.length > 16 * 1024 || bytes.toString('base64url') !== encoded) {
    throw new Error('Acceptance bundle authorization is invalid');
  }
  const envelope = JSON.parse(bytes.toString('utf8'));
  if (Buffer.from(canonicalJson(envelope)).toString('base64url') !== encoded) {
    throw new Error('Acceptance bundle authorization is not canonical');
  }
  const payload = envelope?.payload;
  if (
    payload?.version !== 1 ||
    payload.purpose !== 'talking-quill/windows-installed-acceptance-bundle' ||
    payload.bundleUrl !== input.bundleUrl ||
    payload.bundleSha256 !== input.bundleSha256 ||
    payload.architecture !== input.architecture ||
    !/^[0-9a-f]{64}$/u.test(payload.producerArtifactSetIdentity ?? '') ||
    (input.producerArtifactSetIdentity !== undefined &&
      payload.producerArtifactSetIdentity !== input.producerArtifactSetIdentity) ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceRevision ?? '') ||
    (input.sourceRevision !== undefined && payload.sourceRevision !== input.sourceRevision) ||
    (input.manifestPublicKeySpkiBase64url !== undefined &&
      payload.manifestPublicKeySpkiBase64url !== input.manifestPublicKeySpkiBase64url) ||
    typeof payload.manifestPublicKeySpkiBase64url !== 'string' ||
    !/^[A-Za-z0-9_-]+$/u.test(payload.manifestPublicKeySpkiBase64url) ||
    payload.manifestPublicKeySpkiBase64url.length > 256 ||
    !Number.isSafeInteger(payload.issuedAtMs) ||
    !Number.isSafeInteger(payload.notBeforeMs) ||
    !Number.isSafeInteger(payload.expiresAtMs) ||
    !Number.isSafeInteger(payload.maxTotalRunMs) ||
    payload.notBeforeMs < payload.issuedAtMs ||
    payload.expiresAtMs <= payload.notBeforeMs ||
    payload.maxTotalRunMs <= 0 ||
    payload.maxTotalRunMs > 80 * 60 * 1_000 ||
    payload.expiresAtMs - payload.notBeforeMs < payload.maxTotalRunMs ||
    input.nowMs < payload.notBeforeMs ||
    input.nowMs > Math.min(payload.expiresAtMs, payload.notBeforeMs + payload.maxTotalRunMs)
  ) {
    throw new Error('Acceptance bundle authorization binding is invalid');
  }
  const der = Buffer.from(input.publicKeySpkiBase64url, 'base64url');
  if (der.toString('base64url') !== input.publicKeySpkiBase64url) {
    throw new Error('Acceptance bundle public key is invalid');
  }
  const key = createPublicKey({ key: der, format: 'der', type: 'spki' });
  if (
    key.asymmetricKeyType !== 'ec' ||
    key.asymmetricKeyDetails?.namedCurve !== 'prime256v1' ||
    !verify(
      'sha256',
      Buffer.from(canonicalJson(payload)),
      {
        key,
        dsaEncoding: 'ieee-p1363',
      },
      Buffer.from(envelope.signatureBase64url ?? '', 'base64url'),
    )
  ) {
    throw new Error('Acceptance bundle authorization signature is invalid');
  }
  return Object.freeze(payload);
}

function canonicalJson(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean') {
    return JSON.stringify(value);
  }
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) throw new Error('Canonical number is invalid');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  if (typeof value !== 'object') throw new Error('Canonical value is invalid');
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`;
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  verifyWindowsAcceptanceAuthorization({
    authorizationBase64url: process.env.ACCEPTANCE_BUNDLE_AUTHORIZATION,
    publicKeySpkiBase64url: process.env.ACCEPTANCE_BUNDLE_PUBLIC_KEY_SPKI_BASE64URL,
    bundleUrl: process.env.ACCEPTANCE_BUNDLE_URL,
    bundleSha256: process.env.ACCEPTANCE_BUNDLE_SHA256,
    architecture: process.env.ACCEPTANCE_ARCHITECTURE,
    sourceRevision: process.env.ACCEPTANCE_SOURCE_REVISION,
    manifestPublicKeySpkiBase64url: process.env.ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL,
    producerArtifactSetIdentity: process.env.ACCEPTANCE_PRODUCER_ARTIFACT_SET_IDENTITY,
    nowMs: Date.now(),
  });
}
