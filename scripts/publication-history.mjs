import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { verifyPublicationEnvelope } from './publication-manifest.mjs';

function semver(tag) {
  const match = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.exec(tag);
  if (match === null) throw new Error(`Published release tag is not canonical semver: ${tag}`);
  const parts = match.slice(1).map(Number);
  if (parts.some((part) => !Number.isSafeInteger(part)))
    throw new Error(`Published release tag exceeds safe semver bounds: ${tag}`);
  return parts;
}

function compareVersion(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

export async function verifyPublicationHistory({
  candidatePath,
  historyIndexPath,
  repository,
  publicKeyPath,
  expectedCandidateTag,
  expectedCandidateSequence,
}) {
  const candidate = JSON.parse(await readFile(candidatePath, 'utf8'));
  await verifyPublicationEnvelope({
    envelope: candidate,
    repository,
    tag: expectedCandidateTag,
    publicKeyPath,
  });
  if (candidate.payload.sequence !== Number(expectedCandidateSequence))
    throw new Error('Candidate publication sequence does not match the protected request');
  const history = JSON.parse(await readFile(historyIndexPath, 'utf8'));
  if (!Array.isArray(history)) throw new Error('Publication history index must be an array');
  const releaseIds = new Set();
  const releaseTags = new Set();
  const sequences = new Set();
  const runs = new Set();
  let highestSequence = 0;
  let highestVersion = [-1, -1, -1];
  for (const release of history) {
    if (
      release === null ||
      typeof release !== 'object' ||
      release.draft !== false ||
      release.immutable !== true ||
      !Number.isSafeInteger(release.id) ||
      release.id <= 0 ||
      typeof release.tag !== 'string' ||
      typeof release.manifestPath !== 'string'
    )
      throw new Error('Publication history release metadata is invalid');
    if (releaseIds.has(release.id) || releaseTags.has(release.tag))
      throw new Error('Publication history release is duplicated');
    releaseIds.add(release.id);
    releaseTags.add(release.tag);
    const envelope = JSON.parse(await readFile(resolve(release.manifestPath), 'utf8'));
    await verifyPublicationEnvelope({ envelope, repository, tag: release.tag, publicKeyPath });
    const { sequence, workflowRunId } = envelope.payload;
    if (sequences.has(sequence)) throw new Error('Published release sequence is duplicated');
    if (runs.has(workflowRunId)) throw new Error('Published workflow run is replayed');
    sequences.add(sequence);
    runs.add(workflowRunId);
    highestSequence = Math.max(highestSequence, sequence);
    const version = semver(release.tag);
    if (compareVersion(version, highestVersion) > 0) highestVersion = version;
  }
  if (runs.has(candidate.payload.workflowRunId) || sequences.has(candidate.payload.sequence))
    throw new Error('Candidate publication identity has already been accepted');
  if (candidate.payload.sequence <= highestSequence)
    throw new Error('Candidate publication sequence does not advance immutable history');
  if (compareVersion(semver(candidate.payload.tag), highestVersion) <= 0)
    throw new Error('Candidate semantic version does not advance immutable history');
  return { highestSequence, accepted: history.length };
}

function option(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  await verifyPublicationHistory({
    candidatePath: resolve(option('--candidate')),
    historyIndexPath: resolve(option('--history-index')),
    repository: option('--repository'),
    publicKeyPath: resolve(option('--public-key')),
    expectedCandidateTag: option('--candidate-tag'),
    expectedCandidateSequence: option('--candidate-sequence'),
  });
  console.log('Immutable publication history advances monotonically.');
}
