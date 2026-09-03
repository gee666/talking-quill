import { createHash, createPublicKey, verify } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import {
  ACCEPTANCE_MATRIX,
  ACCEPTANCE_REQUEST_SCHEDULE,
} from './windows-installed-acceptance-schedule.mjs';

const evidencePath = resolve(valueAfter('--evidence') ?? '');
const producerResultPath = resolve(valueAfter('--producer-result') ?? '');
const outputPath = resolve(valueAfter('--output') ?? '');
const architecture = valueAfter('--architecture');
const sourceCommit = valueAfter('--source-commit');
const sourceTree = valueAfter('--source-tree');
const runId = valueAfter('--run-id');
const bundleSha256 = valueAfter('--bundle-sha256');
const bundleManifestSha256 = valueAfter('--manifest-sha256');
const bundleAuthorizationSha256 = valueAfter('--authorization-sha256');
const producerArtifactSetIdentity = valueAfter('--producer-artifact-set-identity');
if (
  !['x64', 'arm64'].includes(architecture) ||
  !/^[0-9a-f]{40}$/u.test(sourceCommit ?? '') ||
  !/^[0-9a-f]{40}$/u.test(sourceTree ?? '') ||
  ![
    bundleSha256,
    bundleManifestSha256,
    bundleAuthorizationSha256,
    producerArtifactSetIdentity,
  ].every((value) => /^[0-9a-f]{64}$/u.test(value ?? '')) ||
  !/^[1-9][0-9]*$/u.test(runId ?? '')
) {
  throw new Error('Installed acceptance gate identity is invalid');
}
const bytes = await readFile(evidencePath);
const producerBytes = await readFile(producerResultPath);
const producerText = producerBytes.toString('utf8');
const producerResult = JSON.parse(producerText);
const producerPayload = producerResult?.payload;
const evidence = JSON.parse(bytes.toString('utf8'));
if (
  !producerText.endsWith('\n') ||
  canonicalAcceptanceJson(producerResult) !== producerText.slice(0, -1) ||
  producerPayload?.version !== 1 ||
  producerPayload.purpose !== 'talking-quill/windows-installed-acceptance-producer-result' ||
  producerPayload.sourceCommit !== sourceCommit ||
  producerPayload.sourceTree !== sourceTree ||
  producerPayload.buildId !== evidence.buildId ||
  producerPayload.producerArtifactSetIdentity !== producerArtifactSetIdentity ||
  evidence.producerResult?.sha256 !== sha256(producerBytes) ||
  !verifyProducerSignature(producerResult, evidence.producerResult?.requestPublicKeySpkiBase64url)
) {
  throw new Error('Executed-bundle producer result binding is invalid');
}
const phases = evidence.phases?.map(({ phase }) => phase);
if (
  evidence.schemaVersion !== 2 ||
  evidence.result !== 'passed' ||
  evidence.architecture !== architecture ||
  evidence.sourceCommit !== sourceCommit ||
  evidence.sourceTree !== sourceTree ||
  evidence.bundleSha256 !== bundleSha256 ||
  evidence.bundleManifestSha256 !== bundleManifestSha256 ||
  evidence.bundleAuthorizationSha256 !== bundleAuthorizationSha256 ||
  evidence.producerArtifactSetIdentity !== producerArtifactSetIdentity ||
  !/^[0-9a-f]{64}$/u.test(evidence.buildId ?? '') ||
  !/^[0-9a-f]{64}$/u.test(evidence.candidateInstallerSha256 ?? '') ||
  !['releaseBuildDigest', 'packageLayoutDigest', 'gatewaySha256', 'ownerSha256'].every((name) =>
    /^[0-9a-f]{64}$/u.test(evidence.targetIdentity?.[name] ?? ''),
  ) ||
  !['broker', 'bootstrap', 'launcher'].every(
    (name) =>
      /^[0-9a-f]{64}$/u.test(evidence.acceptanceNative?.[name]?.sha256 ?? '') &&
      Number.isSafeInteger(evidence.acceptanceNative[name].bytes) &&
      evidence.acceptanceNative[name].bytes > 0,
  ) ||
  canonicalAcceptanceJson(phases) !== canonicalAcceptanceJson(ACCEPTANCE_MATRIX) ||
  evidence.preflight?.manifestSignatureVerified !== true ||
  evidence.preflight?.requestSignaturesVerified !== ACCEPTANCE_REQUEST_SCHEDULE.length ||
  !/^[0-9a-f]{64}$/u.test(evidence.preflight?.validationKeySha256 ?? '') ||
  evidence.preflight?.faultRecordsVerified !== 10 ||
  !/^[0-9a-f]{64}$/u.test(evidence.preflight?.faultChainHeadSha256 ?? '') ||
  evidence.phases.some(({ result }) => result?.result !== 'passed') ||
  evidence.phases.find(({ phase }) => phase === 'manual-physical-observation')?.result
    ?.physicalObservation !== true ||
  evidence.phases.find(({ phase }) => phase === 'residue')?.result?.zeroResidue !== true
) {
  throw new Error('Installed acceptance evidence is incomplete or failed');
}
const artifactHashes = Object.values(evidence.artifacts ?? {}).flatMap((artifact) =>
  artifact?.installer === undefined
    ? Object.values(artifact ?? {}).map((entry) => entry?.installer?.sha256)
    : [artifact.installer.sha256],
);
if (artifactHashes.length === 0 || artifactHashes.some((hash) => !/^[0-9a-f]{64}$/u.test(hash))) {
  throw new Error('Installed acceptance artifact identity is incomplete');
}
const summary = {
  schemaVersion: 1,
  purpose: 'talking-quill/windows-installed-acceptance-gate',
  result: 'passed',
  repository: process.env.GITHUB_REPOSITORY ?? 'local',
  workflow: '.github/workflows/windows-installed-acceptance.yml',
  runId,
  architecture,
  sourceCommit,
  sourceTree,
  buildId: evidence.buildId,
  candidateInstallerSha256: evidence.candidateInstallerSha256,
  targetReleaseBuildDigest: evidence.targetIdentity.releaseBuildDigest,
  targetPackageLayoutDigest: evidence.targetIdentity.packageLayoutDigest,
  targetGatewaySha256: evidence.targetIdentity.gatewaySha256,
  targetOwnerSha256: evidence.targetIdentity.ownerSha256,
  bundleSha256,
  bundleManifestSha256,
  bundleAuthorizationSha256,
  producerArtifactSetIdentity,
  evidenceSha256: sha256(bytes),
  validationKeySha256: evidence.preflight.validationKeySha256,
  phaseCount: phases.length,
  brokerSha256: evidence.acceptanceNative.broker.sha256,
  bootstrapSha256: evidence.acceptanceNative.bootstrap.sha256,
  launcherSha256: evidence.acceptanceNative.launcher.sha256,
  producerResultSha256: sha256(producerBytes),
  artifactSha256: [...new Set(artifactHashes)].sort(),
};
await writeFile(outputPath, `${canonicalAcceptanceJson(summary)}\n`, { flag: 'wx', mode: 0o600 });
console.log(canonicalAcceptanceJson(summary));

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}
function verifyProducerSignature(envelope, publicKeySpkiBase64url) {
  try {
    const key = createPublicKey({
      key: Buffer.from(publicKeySpkiBase64url, 'base64url'),
      format: 'der',
      type: 'spki',
    });
    return verify(
      'sha256',
      Buffer.from(canonicalAcceptanceJson(envelope.payload)),
      { key, dsaEncoding: 'ieee-p1363' },
      Buffer.from(envelope.signatureBase64url ?? '', 'base64url'),
    );
  } catch {
    return false;
  }
}
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
