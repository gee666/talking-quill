import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { lstatSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { load } from 'js-yaml';
import {
  currentSourceTreeHash,
  validateArtifactProvenanceManifest,
} from './artifact-provenance.mjs';
import { parseReleaseTag, releaseConfig, repositoryRoot } from './release-config.mjs';
import { sealReleaseManifest } from './release-manifest.mjs';

const args = process.argv.slice(2).filter((argument) => argument !== '--');
if (args.length !== 2) throw new Error('Usage: assemble-release <tag> <artifact-directory>.');
const { tag, version } = parseReleaseTag(args[0]);
const directory = resolve(args[1]);
const checkedOutCommit = git(['rev-parse', 'HEAD^{commit}']).trim();
const checkedOutTree = git(['rev-parse', 'HEAD^{tree}']).trim();
const commit = process.env.TALKING_QUILL_RELEASE_COMMIT ?? checkedOutCommit;
const sourceTree = process.env.TALKING_QUILL_RELEASE_TREE ?? checkedOutTree;
if (!/^[0-9a-f]{40}$/u.test(commit) || commit !== checkedOutCommit) {
  throw new Error('Release source commit is invalid or does not match the checkout.');
}
if (!/^[0-9a-f]{40}$/u.test(sourceTree) || sourceTree !== checkedOutTree) {
  throw new Error('Release source tree is invalid or does not match the checkout.');
}

const architecture = process.env.TALKING_QUILL_PACKAGE_ARCH ?? 'x64';
if (!['x64', 'arm64'].includes(architecture)) throw new Error('Invalid Windows architecture');
const freshInstaller = `Talking-Quill-${version}-win-${architecture}-setup.exe`;
const updateInstaller = `Talking-Quill-${version}-win-${architecture}-update.exe`;
const freshTrustRoot = process.env.TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT === '1';
const expectedInputs = (
  freshTrustRoot
    ? [freshInstaller, `provenance-win-${architecture}-setup.json`, 'THIRD_PARTY_NOTICES.txt']
    : [
        freshInstaller,
        updateInstaller,
        `${updateInstaller}.blockmap`,
        `latest-${architecture}.yml`,
        `provenance-win-${architecture}-setup.json`,
        `provenance-win-${architecture}-update.json`,
        `release-identity-win-${architecture}.json`,
        'THIRD_PARTY_NOTICES.txt',
      ]
).sort();
const actualInputs = readdirSync(directory).sort();
if (JSON.stringify(actualInputs) !== JSON.stringify(expectedInputs)) {
  throw new Error(`Release input allowlist mismatch: ${actualInputs.join(', ')}`);
}
for (const name of actualInputs) {
  const metadata = lstatSync(resolve(directory, name));
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Release input must be a regular file: ${name}`);
  }
}

if (freshTrustRoot) {
  const provenanceName = `provenance-win-${architecture}-setup.json`;
  const provenance = JSON.parse(readFileSync(resolve(directory, provenanceName), 'utf8'));
  validateArtifactProvenanceManifest(provenance);
  const sourceTreeSha256 = await currentSourceTreeHash();
  const finalEntries = provenance.entries.filter((entry) => entry.role === 'final-artifact');
  if (
    provenance.sourceCommit !== commit ||
    provenance.sourceTree !== sourceTree ||
    provenance.sourceTreeSha256 !== sourceTreeSha256 ||
    provenance.package.version !== version ||
    provenance.package.platform !== 'win' ||
    provenance.package.arch !== architecture ||
    finalEntries.length !== 1 ||
    finalEntries[0].path.split('/').at(-1) !== freshInstaller ||
    finalEntries[0].sha256 !== sha256(resolve(directory, freshInstaller))
  ) {
    throw new Error(`Windows ${architecture} fresh trust-root provenance identity mismatch`);
  }
  const assets = expectedInputs.map((name) => {
    const path = resolve(directory, name);
    return { name, bytes: lstatSync(path).size, sha256: sha256(path) };
  });
  const manifest = sealReleaseManifest({
    schemaVersion: 2,
    repository: releaseConfig.repository,
    tag,
    version,
    sourceCommit: commit,
    sourceTree,
    platform: 'win',
    architecture,
    promotable: true,
    workflowRunId:
      process.env.TALKING_QUILL_DETERMINISTIC === '1' ? null : (process.env.GITHUB_RUN_ID ?? null),
    generatedAt:
      process.env.TALKING_QUILL_DETERMINISTIC === '1' || process.env.GITHUB_RUN_ID === undefined
        ? null
        : new Date().toISOString(),
    provenance: [
      {
        name: provenanceName,
        platform: 'win',
        arch: architecture,
        mode: 'setup',
        sourceTree,
        sourceTreeSha256,
      },
    ],
    assets,
  });
  writeFileSync(
    resolve(directory, 'release-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
    'utf8',
  );
  console.log(
    `Assembled ${tag}: ${String(assets.length)} validated Windows ${architecture} fresh trust-root inputs plus release-manifest.json.`,
  );
} else {
  const provenanceName = `provenance-win-${architecture}-update.json`;
  const freshProvenanceName = `provenance-win-${architecture}-setup.json`;
  const identityName = `release-identity-win-${architecture}.json`;
  const provenance = JSON.parse(readFileSync(resolve(directory, provenanceName), 'utf8'));
  try {
    validateArtifactProvenanceManifest(provenance);
  } catch (error) {
    throw new Error(`Provenance schema mismatch: ${provenanceName}`, { cause: error });
  }
  const freshProvenance = JSON.parse(readFileSync(resolve(directory, freshProvenanceName), 'utf8'));
  validateArtifactProvenanceManifest(freshProvenance);
  const finalEntry = [...provenance.entries, ...freshProvenance.entries].filter(
    (entry) => entry.role === 'final-artifact',
  );
  const sourceTreeSha256 = await currentSourceTreeHash();
  if (
    provenance.sourceCommit !== commit ||
    provenance.sourceTree !== sourceTree ||
    freshProvenance.sourceCommit !== commit ||
    freshProvenance.sourceTree !== sourceTree ||
    freshProvenance.sourceTreeSha256 !== sourceTreeSha256 ||
    freshProvenance.package.version !== version ||
    freshProvenance.package.platform !== 'win' ||
    freshProvenance.package.arch !== architecture ||
    provenance.sourceTreeSha256 !== sourceTreeSha256 ||
    provenance.package.version !== version ||
    provenance.package.platform !== 'win' ||
    provenance.package.arch !== architecture ||
    ![freshInstaller, updateInstaller].every((name) =>
      finalEntry.some(
        (entry) =>
          entry.path.split('/').at(-1) === name &&
          entry.sha256 === sha256(resolve(directory, name)),
      ),
    )
  ) {
    throw new Error(`Windows ${architecture} provenance identity mismatch`);
  }

  const identity = JSON.parse(readFileSync(resolve(directory, identityName), 'utf8'));
  const updater = load(readFileSync(resolve(directory, `latest-${architecture}.yml`), 'utf8'));
  if (
    identity?.schemaVersion !== 1 ||
    identity?.version !== version ||
    identity?.platform !== 'win' ||
    identity?.architecture !== architecture ||
    identity?.packageSha256 !== sha256(resolve(directory, updateInstaller)) ||
    identity?.predecessor === null ||
    identity?.authorization?.scheme !== 'p256-sha256-v1' ||
    !/^[0-9a-f]{64}$/u.test(identity?.authorization?.verificationKeySha256 ?? '') ||
    typeof identity?.authorization?.signature !== 'string' ||
    identity.authorization.signature.length < 8 ||
    identity.predecessor.platform !== 'win' ||
    identity.predecessor.architecture !== architecture ||
    !Array.isArray(identity?.roles) ||
    JSON.stringify(identity.roles.map((role) => role?.role)) !==
      JSON.stringify(['gateway', 'owner', 'recovery-launcher']) ||
    identity.roles.filter((role) => role?.suppressionCapable === true).length !== 1 ||
    JSON.stringify(updater?.talkingQuillRelease) !== JSON.stringify(identity)
  ) {
    throw new Error(`Windows ${architecture} updater identity mismatch`);
  }

  const assets = expectedInputs.map((name) => {
    const path = resolve(directory, name);
    return { name, bytes: lstatSync(path).size, sha256: sha256(path) };
  });
  const manifest = sealReleaseManifest({
    schemaVersion: 2,
    repository: releaseConfig.repository,
    tag,
    version,
    sourceCommit: commit,
    sourceTree,
    platform: 'win',
    architecture,
    promotable: true,
    workflowRunId:
      process.env.TALKING_QUILL_DETERMINISTIC === '1' ? null : (process.env.GITHUB_RUN_ID ?? null),
    generatedAt:
      process.env.TALKING_QUILL_DETERMINISTIC === '1' || process.env.GITHUB_RUN_ID === undefined
        ? null
        : new Date().toISOString(),
    provenance: [
      {
        name: freshProvenanceName,
        platform: 'win',
        arch: architecture,
        mode: 'setup',
        sourceTree,
        sourceTreeSha256,
      },
      {
        name: provenanceName,
        platform: 'win',
        arch: architecture,
        mode: 'update',
        sourceTree,
        sourceTreeSha256,
      },
    ],
    assets,
  });
  writeFileSync(
    resolve(directory, 'release-manifest.json'),
    `${JSON.stringify(manifest, null, 2)}\n`,
    'utf8',
  );
  console.log(
    `Assembled ${tag}: ${String(assets.length)} validated Windows ${architecture} inputs plus release-manifest.json.`,
  );
}

function sha256(path) {
  return createHash('sha256').update(readFileSync(path)).digest('hex');
}
function git(arguments_) {
  return execFileSync('git', arguments_, { cwd: repositoryRoot, encoding: 'utf8' });
}
