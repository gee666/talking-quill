import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';

const hex = (value, bytes) =>
  typeof value === 'string' && new RegExp(`^[0-9a-f]{${String(bytes * 2)}}$`, 'u').test(value)
    ? Buffer.from(value, 'hex')
    : null;
const le32 = (value) => {
  const result = Buffer.alloc(4);
  result.writeUInt32LE(value);
  return result;
};
const hash = (bytes) => createHash('sha256').update(bytes).digest();

export async function verifyWindowsInstallerSuccessEvidence({
  evidencePath,
  installerPath,
  provenancePath,
  architecture,
  operation,
}) {
  const [source, installer, provenanceSource] = await Promise.all([
    readFile(evidencePath, 'utf8'),
    readFile(installerPath),
    readFile(provenancePath),
  ]);
  const evidence = JSON.parse(source);
  const provenance = JSON.parse(provenanceSource);
  validateArtifactProvenanceManifest(provenance);
  const receipt = evidence.nativeAuthenticationReceipt;
  const installerHash = hash(installer);
  const fields = {
    package: hex(receipt?.packageSha256, 32),
    peer: hex(receipt?.peerBinding, 32),
    transcript: hex(receipt?.transcriptSha256, 32),
    challenge: hex(receipt?.observerChallenge, 32),
    nonce: hex(receipt?.nonce, 32),
    controllerPublic: hex(receipt?.controllerPublicKey, 65),
    workerPublic: hex(receipt?.workerPublicKey, 65),
    request: hex(receipt?.request, 6),
    workerProof: hex(receipt?.workerProof, 32),
    controllerProof: hex(receipt?.controllerProof, 32),
    evidencePublic: hex(receipt?.evidencePublicKey, 65),
    signature: hex(receipt?.evidenceSignature, 64),
  };
  if (Object.values(fields).some((value) => value === null))
    throw new Error('Setup transcript receipt fields are invalid');
  const transcript = Buffer.concat([
    Buffer.from('TalkingQuill/setup-authenticated-transcript/v1'),
    fields.nonce,
    fields.controllerPublic,
    fields.workerPublic,
    fields.peer,
    fields.request,
    fields.workerProof,
    fields.controllerProof,
  ]);
  if (!hash(transcript).equals(fields.transcript) || !fields.package.equals(installerHash))
    throw new Error('Setup transcript digest or package binding is invalid');
  const signed = Buffer.concat([
    Buffer.from('TalkingQuill/setup-evidence-signature/v1'),
    fields.challenge,
    le32(receipt.controllerPid),
    le32(receipt.workerPid),
    fields.package,
    fields.peer,
    fields.transcript,
    fields.nonce,
    fields.controllerPublic,
    fields.workerPublic,
    fields.request,
    fields.workerProof,
    fields.controllerProof,
  ]);
  const spki = Buffer.concat([
    Buffer.from('3059301306072a8648ce3d020106082a8648ce3d030107034200', 'hex'),
    fields.evidencePublic,
  ]);
  const key = createPublicKey({ key: spki, format: 'der', type: 'spki' });
  if (!verify('sha256', signed, { key, dsaEncoding: 'ieee-p1363' }, fields.signature))
    throw new Error('Setup transcript P-256 signature is invalid');
  const identities = evidence.processIdentities;
  const authenticatedPids = [...(evidence.authenticatedSetupPids ?? [])].sort(
    (left, right) => left - right,
  );
  const receiptPids = [receipt.controllerPid, receipt.workerPid].sort(
    (left, right) => left - right,
  );
  const controller = identities?.find(({ pid }) => pid === receipt.controllerPid);
  const worker = identities?.find(({ pid }) => pid === receipt.workerPid);
  const finalEntries = provenance.entries.filter(({ role }) => role === 'final-artifact');
  if (
    evidence.passed !== true ||
    evidence.installedIdentityBound !== true ||
    evidence.registrationsExact !== true ||
    evidence.terminalTopology !== true ||
    !Array.isArray(evidence.interpreterProcessStarts) ||
    evidence.interpreterProcessStarts.length !== 0 ||
    !Array.isArray(evidence.observerErrors) ||
    evidence.observerErrors.length !== 0 ||
    evidence.pipeObserved !== true ||
    evidence.exitCode !== 0 ||
    receipt.schemaVersion !== 2 ||
    authenticatedPids.length !== 2 ||
    authenticatedPids.some((pid, index) => pid !== receiptPids[index]) ||
    identities.length !== 2 ||
    evidence.packageMode !== (operation === 'fresh' ? 'fresh' : 'update') ||
    evidence.architecture !== architecture ||
    evidence.operation !== operation ||
    evidence.installerSha256 !== installerHash.toString('hex') ||
    evidence.sourceCommit !== provenance.sourceCommit ||
    evidence.sourceTree !== provenance.sourceTree ||
    provenance.package.arch !== architecture ||
    !controller ||
    !worker ||
    worker.parentPid !== controller.pid ||
    controller.imageSha256 !== evidence.installerSha256 ||
    worker.imageSha256 !== evidence.installerSha256 ||
    typeof controller.userSid !== 'string' ||
    controller.userSid.length === 0 ||
    typeof controller.logonId !== 'string' ||
    controller.logonId.length === 0 ||
    controller.userSid !== worker.userSid ||
    controller.sessionId !== worker.sessionId ||
    controller.logonId !== worker.logonId ||
    finalEntries.length !== 1 ||
    basename(finalEntries[0].path) !== basename(installerPath) ||
    finalEntries[0].sha256 !== evidence.installerSha256 ||
    finalEntries[0].size !== installer.length
  )
    throw new Error('Setup evidence process, source, or provenance binding is invalid');
  return evidence;
}

function argument(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  await verifyWindowsInstallerSuccessEvidence({
    evidencePath: resolve(argument('--evidence')),
    installerPath: resolve(argument('--installer')),
    provenancePath: resolve(argument('--provenance')),
    architecture: argument('--arch'),
    operation: argument('--operation'),
  });
}
