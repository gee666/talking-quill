import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';

const HEX_SIGNATURE = /^[0-9a-f]{128}$/u;
const HEX_SEC1 = /^04[0-9a-f]{128}$/u;
const HEX_SHA256 = /^[0-9a-f]{64}$/u;

export function signAcceptancePayload({
  signerPath,
  privateKeyPath,
  payloadBytes,
  signerSha256,
  signerSourceCommit,
  signerSourceTree,
  brokerPath = resolve(dirname(signerPath), 'talking-quill-windows-acceptance-broker.exe'),
  brokerSha256,
  spawnProcess = spawnSync,
}) {
  if (
    !Buffer.isBuffer(payloadBytes) ||
    payloadBytes.length === 0 ||
    payloadBytes.length > 64 * 1024 ||
    !HEX_SHA256.test(signerSha256 ?? '')
  ) {
    throw new Error('Acceptance signing request is invalid');
  }
  const absoluteBroker = resolve(brokerPath);
  const absoluteSigner = resolve(signerPath);
  const brokerMetadata = lstatSync(absoluteBroker);
  const brokerBytes = readFileSync(absoluteBroker);
  const observedBrokerSha256 = createHash('sha256').update(brokerBytes).digest('hex');
  if (
    !brokerMetadata.isFile() ||
    brokerMetadata.isSymbolicLink() ||
    brokerMetadata.nlink !== 1 ||
    brokerMetadata.size !== brokerBytes.length ||
    (brokerSha256 !== undefined && brokerSha256 !== observedBrokerSha256)
  ) {
    throw new Error('Native acceptance broker identity is invalid');
  }
  const correlation = randomBytes(16).toString('hex');
  const request = {
    version: 1,
    operation: 'sign',
    correlation,
    brokerSha256: observedBrokerSha256,
    brokerBytes: brokerBytes.length,
    signerPath: absoluteSigner,
    signerSha256,
    signerBytes: lstatSync(absoluteSigner).size,
    privateKeyPath: resolve(privateKeyPath),
    payloadHex: payloadBytes.toString('hex'),
    ...(signerSourceCommit === undefined ? {} : { sourceCommit: signerSourceCommit }),
    ...(signerSourceTree === undefined ? {} : { sourceTree: signerSourceTree }),
  };
  const result = spawnProcess(absoluteBroker, [], {
    cwd: resolve('.'),
    env: sanitizedSubprocessEnvironment({
      SystemRoot: process.env.SystemRoot,
      WINDIR: process.env.WINDIR,
    }),
    input: Buffer.from(`${canonicalAcceptanceJson(request)}\n`),
    encoding: 'utf8',
    windowsHide: true,
    timeout: 12_000,
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
