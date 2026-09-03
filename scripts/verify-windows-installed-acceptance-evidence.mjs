import { createHash } from 'node:crypto';
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
if (
  !['x64', 'arm64'].includes(architecture) ||
  !/^[0-9a-f]{40}$/u.test(sourceCommit ?? '') ||
  !/^[0-9a-f]{40}$/u.test(sourceTree ?? '') ||
  !/^[0-9a-f]{64}$/u.test(bundleSha256 ?? '') ||
  !/^[1-9][0-9]*$/u.test(runId ?? '')
) {
  throw new Error('Installed acceptance gate identity is invalid');
}
const bytes = await readFile(evidencePath);
const producerBytes = await readFile(producerResultPath);
const producerResult = JSON.parse(producerBytes.toString('utf8'));
if (
  producerResult.result !== 'passed' ||
  !/^[0-9a-f]{64}$/u.test(producerResult.bundleSha256 ?? '')
) {
  throw new Error('Non-mocked acceptance producer E2E did not pass');
}
const evidence = JSON.parse(bytes.toString('utf8'));
const phases = evidence.phases?.map(({ phase }) => phase);
if (
  evidence.schemaVersion !== 2 ||
  evidence.result !== 'passed' ||
  evidence.architecture !== architecture ||
  evidence.sourceCommit !== sourceCommit ||
  evidence.sourceTree !== sourceTree ||
  evidence.bundleSha256 !== bundleSha256 ||
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
  evidenceSha256: sha256(bytes),
  validationKeySha256: evidence.preflight.validationKeySha256,
  phaseCount: phases.length,
  brokerSha256: evidence.acceptanceNative.broker.sha256,
  bootstrapSha256: evidence.acceptanceNative.bootstrap.sha256,
  launcherSha256: evidence.acceptanceNative.launcher.sha256,
  producerE2eSha256: sha256(producerBytes),
  producerBundleSha256: producerResult.bundleSha256,
  artifactSha256: [...new Set(artifactHashes)].sort(),
};
await writeFile(outputPath, `${canonicalAcceptanceJson(summary)}\n`, { flag: 'wx', mode: 0o600 });
console.log(canonicalAcceptanceJson(summary));

function sha256(value) {
  return createHash('sha256').update(value).digest('hex');
}
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
