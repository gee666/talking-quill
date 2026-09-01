import { createHash, createPrivateKey, createPublicKey, sign, verify } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';

const DOMAIN = Buffer.from('TalkingQuill/windows-real-reboot-acceptance/v1\0');
const SHA256 = /^[0-9a-f]{64}$/u;
const ARCHITECTURES = new Set(['x64', 'arm64']);

function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object')
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`)
      .join(',')}}`;
  return JSON.stringify(value);
}

function validate(payload) {
  const keys = Object.keys(payload).sort().join(',');
  if (
    keys !==
      'architecture,candidateSha256,checkpointSha256,generationAfter,generationBefore,machineIdentity,pendingDeleteSources,postBootIdentity,preBootIdentity,schemaVersion,sourceRevision,terminalGeneration,windowsConsumedPendingDeletes,workflowRunAttempt,workflowRunId' ||
    payload.schemaVersion !== 1 ||
    !ARCHITECTURES.has(payload.architecture) ||
    !SHA256.test(payload.candidateSha256) ||
    !/^[0-9a-f]{40}$/u.test(payload.sourceRevision) ||
    !SHA256.test(payload.checkpointSha256) ||
    !/^[1-9][0-9]*$/u.test(payload.workflowRunId) ||
    !Number.isSafeInteger(payload.workflowRunAttempt) ||
    payload.workflowRunAttempt < 1 ||
    typeof payload.machineIdentity !== 'string' ||
    payload.machineIdentity.length === 0 ||
    typeof payload.preBootIdentity !== 'string' ||
    payload.preBootIdentity.length === 0 ||
    typeof payload.postBootIdentity !== 'string' ||
    payload.postBootIdentity.length === 0 ||
    payload.preBootIdentity === payload.postBootIdentity ||
    !/^[0-9a-f]{32}$/u.test(payload.generationBefore) ||
    !/^[0-9a-f]{32}$/u.test(payload.generationAfter) ||
    payload.generationBefore === payload.generationAfter ||
    !/^[0-9a-f]{32}$/u.test(payload.terminalGeneration) ||
    !Array.isArray(payload.pendingDeleteSources) ||
    payload.pendingDeleteSources.length === 0 ||
    payload.pendingDeleteSources.some(
      (path) =>
        typeof path !== 'string' ||
        !path.endsWith(`.Talking Quill Terminal Cleanup-${payload.terminalGeneration}.exe`),
    ) ||
    new Set(payload.pendingDeleteSources).size !== payload.pendingDeleteSources.length ||
    payload.windowsConsumedPendingDeletes !== true
  ) {
    throw new Error('Real reboot acceptance evidence is invalid');
  }
}

function signedBytes(payload) {
  return Buffer.concat([DOMAIN, Buffer.from(canonical(payload))]);
}

export async function signRebootEvidence(input, output, privateKeyBase64) {
  const payload = JSON.parse(await readFile(input, 'utf8'));
  validate(payload);
  const key = createPrivateKey({
    key: Buffer.from(privateKeyBase64, 'base64'),
    format: 'der',
    type: 'pkcs8',
  });
  const publicKey = createPublicKey(key);
  const signature = sign('sha256', signedBytes(payload), { key, dsaEncoding: 'ieee-p1363' });
  const envelope = {
    schemaVersion: 1,
    payload,
    publicKeySha256: createHash('sha256')
      .update(publicKey.export({ format: 'der', type: 'spki' }))
      .digest('hex'),
    signature: signature.toString('base64url'),
  };
  await writeFile(output, `${canonical(envelope)}\n`, { flag: 'wx' });
}

export async function verifyRebootEvidence(path, publicKeyBase64) {
  const envelope = JSON.parse(await readFile(path, 'utf8'));
  if (Object.keys(envelope).sort().join(',') !== 'payload,publicKeySha256,schemaVersion,signature')
    throw new Error('Real reboot acceptance envelope is invalid');
  validate(envelope.payload);
  const key = createPublicKey({
    key: Buffer.from(publicKeyBase64, 'base64'),
    format: 'der',
    type: 'spki',
  });
  const keyHash = createHash('sha256')
    .update(key.export({ format: 'der', type: 'spki' }))
    .digest('hex');
  if (envelope.schemaVersion !== 1 || envelope.publicKeySha256 !== keyHash)
    throw new Error('Real reboot acceptance key is invalid');
  const signature = Buffer.from(envelope.signature, 'base64url');
  if (
    !verify('sha256', signedBytes(envelope.payload), { key, dsaEncoding: 'ieee-p1363' }, signature)
  )
    throw new Error('Real reboot acceptance signature is invalid');
  return envelope.payload;
}

if (
  process.argv[1] === new URL(import.meta.url).pathname ||
  process.argv[1]?.replaceAll('\\', '/') === new URL(import.meta.url).pathname.slice(1)
) {
  const [command, input, output] = process.argv.slice(2);
  if (command !== 'sign' || !input || !output) throw new Error('Usage: sign INPUT OUTPUT');
  await signRebootEvidence(
    input,
    output,
    process.env.TALKING_QUILL_WINDOWS_REBOOT_ACCEPTANCE_SIGNING_KEY_PKCS8_BASE64 ?? '',
  );
}
