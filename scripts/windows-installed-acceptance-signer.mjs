import { randomBytes } from 'node:crypto';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { launchVerifiedChildSync } from './windows-verified-child-launcher.mjs';

const HEX_SIGNATURE = /^[0-9a-f]{128}$/u;
const HEX_SEC1 = /^04[0-9a-f]{128}$/u;
const HEX_SHA256 = /^[0-9a-f]{64}$/u;

export function signAcceptancePayload({
  signerPath,
  privateKeyPath,
  payloadBytes,
  signerSha256,
  signerBytes,
  signerSourceCommit,
  signerSourceTree,
  brokerPath = resolve(dirname(signerPath), 'talking-quill-windows-acceptance-broker.exe'),
  brokerSha256,
  brokerBytes,
  bootstrapIdentity,
  launchProcess = launchVerifiedChildSync,
}) {
  if (
    !Buffer.isBuffer(payloadBytes) ||
    payloadBytes.length === 0 ||
    payloadBytes.length > 64 * 1024 ||
    !HEX_SHA256.test(signerSha256 ?? '') ||
    !Number.isSafeInteger(signerBytes) ||
    signerBytes <= 0 ||
    !HEX_SHA256.test(brokerSha256 ?? '') ||
    !Number.isSafeInteger(brokerBytes) ||
    brokerBytes <= 0
  ) {
    throw new Error('Acceptance signing request is invalid');
  }
  const absoluteBroker = resolve(brokerPath);
  const absoluteSigner = resolve(signerPath);
  const correlation = randomBytes(16).toString('hex');
  const request = {
    version: 1,
    operation: 'sign',
    correlation,
    brokerSha256,
    brokerBytes,
    signerPath: absoluteSigner,
    signerSha256,
    signerBytes,
    privateKeyPath: resolve(privateKeyPath),
    payloadHex: payloadBytes.toString('hex'),
    ...(signerSourceCommit === undefined ? {} : { sourceCommit: signerSourceCommit }),
    ...(signerSourceTree === undefined ? {} : { sourceTree: signerSourceTree }),
  };
  const result = launchProcess({
    bootstrap: bootstrapIdentity,
    child: { path: absoluteBroker, sha256: brokerSha256, bytes: brokerBytes },
    timeoutMs: 12_000,
    input: Buffer.from(`${canonicalAcceptanceJson(request)}\n`),
    maxBuffer: 4 * 1024,
  });
  if (
    result?.error !== undefined ||
    (result?.signal !== null && result?.signal !== undefined) ||
    result?.status !== 0 ||
    typeof result.stderr !== 'string' ||
    result.stderr !== '' ||
    typeof result.stdout !== 'string' ||
    result.stdout.length > 2048
  ) {
    throw new Error('Native acceptance signing broker failed');
  }
  let response;
  try {
    response = JSON.parse(result.stdout);
  } catch {
    throw new Error('Native acceptance signing broker result is invalid');
  }
  const keys = Object.keys(response).sort().join(',');
  if (
    keys !==
      'correlation,creationIdentityMatches,parentIdentityMatches,processHashMatches,processIdentityMatches,publicKeySec1Hex,result,retainedIdentityMatches,signatureHex,signerBytes,signerSha256,version' ||
    response.version !== 1 ||
    response.correlation !== correlation ||
    response.result !== 'passed' ||
    response.signerSha256 !== signerSha256 ||
    response.signerBytes !== request.signerBytes ||
    response.retainedIdentityMatches !== true ||
    response.processIdentityMatches !== true ||
    response.processHashMatches !== true ||
    response.parentIdentityMatches !== true ||
    response.creationIdentityMatches !== true ||
    !HEX_SIGNATURE.test(response.signatureHex ?? '') ||
    !HEX_SEC1.test(response.publicKeySec1Hex ?? '')
  ) {
    throw new Error('Native acceptance signing broker result is invalid');
  }
  const sec1 = Buffer.from(response.publicKeySec1Hex, 'hex');
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    sec1,
  ]);
  return Object.freeze({
    signatureBase64url: Buffer.from(response.signatureHex, 'hex').toString('base64url'),
    publicKeySpkiBase64url: spki.toString('base64url'),
  });
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  throw new Error('Use the Windows acceptance broker; this module only validates its result');
}
