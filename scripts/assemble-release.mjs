import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, resolve } from 'node:path';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';
import { parseReleaseTag, releaseConfig, repositoryRoot } from './release-config.mjs';

const args = process.argv.slice(2).filter((argument) => argument !== '--');
if (args.length !== 2) throw new Error('Usage: assemble-release <tag> <artifact-directory>.');
const { tag, version } = parseReleaseTag(args[0]);
const directory = resolve(args[1]);
const checkedOutCommit = git(['rev-parse', 'HEAD^{commit}']).trim();
const commit = process.env.TALKING_QUILL_RELEASE_COMMIT ?? checkedOutCommit;
if (!/^[0-9a-f]{40}$/u.test(commit) || commit !== checkedOutCommit) {
  throw new Error('Release source commit is invalid or does not match the checkout.');
}
const finalArtifacts = [
  `Talking-Quill-${version}-win-x64.exe`,
  `Talking-Quill-${version}-win-arm64.exe`,
  `Talking-Quill-${version}-mac-x64.dmg`,
  `Talking-Quill-${version}-mac-x64.zip`,
  `Talking-Quill-${version}-mac-arm64.dmg`,
  `Talking-Quill-${version}-mac-arm64.zip`,
];
const provenanceNames = [
  'provenance-win-x64.json',
  'provenance-win-arm64.json',
  'provenance-mac-x64.json',
  'provenance-mac-arm64.json',
];
const expectedInputs = [...finalArtifacts, ...provenanceNames, 'THIRD_PARTY_NOTICES.txt'].sort();
const actualInputs = readdirSync(directory).sort();
if (JSON.stringify(actualInputs) !== JSON.stringify(expectedInputs)) {
  throw new Error(`Release input allowlist mismatch: ${actualInputs.join(', ')}`);
}
for (const name of actualInputs) {
  const metadata = lstatSync(resolve(directory, name));
  if (!metadata.isFile() || metadata.isSymbolicLink())
    throw new Error(`Release input must be a regular file: ${name}`);
}
const provenance = provenanceNames.map((name) => {
  const value = JSON.parse(readFileSync(resolve(directory, name), 'utf8'));
  const expectedIdentity = /^provenance-(win|mac)-(x64|arm64)\.json$/u.exec(name);
  try {
    validateArtifactProvenanceManifest(value);
  } catch (error) {
    throw new Error(`Provenance schema mismatch: ${name}`, { cause: error });
  }
  if (
    value.sourceCommit !== commit ||
    value.package.version !== version ||
    value.package.platform !== expectedIdentity?.[1] ||
    value.package.arch !== expectedIdentity?.[2]
  )
    throw new Error(`Provenance identity mismatch: ${name}`);
  const entries = value.entries.filter((entry) => entry.role === 'final-artifact');
  const identityArtifacts = finalArtifacts.filter((artifact) =>
    artifact.includes(`-${expectedIdentity?.[1]}-${expectedIdentity?.[2]}.`),
  );
  const provenanceArtifacts = entries.map((entry) => basename(entry.path)).sort();
  if (JSON.stringify(provenanceArtifacts) !== JSON.stringify(identityArtifacts.sort())) {
    throw new Error(`Provenance final-artifact allowlist mismatch: ${name}`);
  }
  for (const entry of entries) {
    const artifactName = basename(entry.path);
    if (
      !finalArtifacts.includes(artifactName) ||
      sha256(resolve(directory, artifactName)) !== entry.sha256
    ) {
      throw new Error(`Provenance artifact hash mismatch: ${name}:${artifactName}`);
    }
  }
  return {
    name,
    platform: value.package.platform,
    arch: value.package.arch,
    sourceTreeSha256: value.sourceTreeSha256,
  };
});
if (new Set(provenance.map(({ sourceTreeSha256 }) => sourceTreeSha256)).size !== 1) {
  throw new Error('All platform provenance records must bind to one identical source tree.');
}
const assets = expectedInputs.map((name) => {
  const path = resolve(directory, name);
  return { name, bytes: lstatSync(path).size, sha256: sha256(path) };
});
const manifest = {
  schemaVersion: 1,
  repository: releaseConfig.repository,
  tag,
  version,
  sourceCommit: commit,
  workflowRunId:
    process.env.TALKING_QUILL_DETERMINISTIC === '1' ? null : (process.env.GITHUB_RUN_ID ?? null),
  generatedAt:
    process.env.TALKING_QUILL_DETERMINISTIC === '1' || process.env.GITHUB_RUN_ID === undefined
      ? null
      : new Date().toISOString(),
  provenance,
  assets,
};
writeFileSync(
  resolve(directory, 'release-manifest.json'),
  `${JSON.stringify(manifest, null, 2)}\n`,
  'utf8',
);
console.log(`Assembled ${tag}: ${assets.length} validated inputs plus release-manifest.json.`);

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
function git(arguments_) {
  return execFileSync('git', arguments_, { cwd: repositoryRoot, encoding: 'utf8' });
}
