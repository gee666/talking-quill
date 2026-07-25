import { spawnSync } from 'node:child_process';
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import {
  parseReleaseTag,
  releaseConfig,
  releaseNotesPath,
  repositoryRoot,
} from './release-config.mjs';

const arguments_ = process.argv.slice(2).filter((argument) => argument !== '--');
const dryRun = arguments_.includes('--dry-run');
const positional = arguments_.filter((argument) => argument !== '--dry-run');
if (positional.length > 1)
  throw new Error('Expected exactly one release tag and optional --dry-run.');
const rawTag = positional[0] ?? process.env.GITHUB_REF_NAME;
const { tag, version: expected } = parseReleaseTag(rawTag ?? '');
const repository = process.env.GITHUB_REPOSITORY;
if (repository !== undefined && repository !== releaseConfig.repository) {
  throw new Error(
    `Release workflow is restricted to ${releaseConfig.repository}; received ${repository}.`,
  );
}
const rootVersion = JSON.parse(
  readFileSync(resolve(repositoryRoot, 'package.json'), 'utf8'),
).version;
const appVersion = JSON.parse(
  readFileSync(resolve(repositoryRoot, 'app/package.json'), 'utf8'),
).version;
const cargo = readFileSync(resolve(repositoryRoot, 'helper/Cargo.toml'), 'utf8').match(
  /^version = "([^"]+)"$/mu,
)?.[1];
if ([rootVersion, appVersion, cargo].some((version) => version !== expected)) {
  throw new Error(
    `Tag ${tag} does not match root=${rootVersion}, app=${appVersion}, helper=${cargo}.`,
  );
}
const notesPath = releaseNotesPath(expected);
if (!existsSync(resolve(repositoryRoot, notesPath)))
  throw new Error(`Per-tag release notes are missing: ${notesPath}`);
const checkedOutCommit = git(['rev-parse', 'HEAD^{commit}']).trim();
let releaseCommit = checkedOutCommit;
if (!dryRun) {
  const type = git(['cat-file', '-t', `refs/tags/${tag}`]).trim();
  if (type !== 'tag')
    throw new Error('A real release requires an annotated tag; lightweight tags are rejected.');
  if (releaseConfig.releaseTagSignerFingerprints.length === 0) {
    throw new Error(
      'No approved release-tag signer fingerprint is configured; publication is blocked.',
    );
  }
  const verification = git(['verify-tag', '--raw', tag], true);
  const signer = /\[GNUPG:\] VALIDSIG ([0-9A-F]{40,64})\b/u.exec(verification)?.[1];
  if (signer === undefined || !releaseConfig.releaseTagSignerFingerprints.includes(signer)) {
    throw new Error('Release tag signature does not match an approved fingerprint.');
  }
  releaseCommit = git(['rev-parse', `refs/tags/${tag}^{commit}`]).trim();
  if (releaseCommit !== checkedOutCommit)
    throw new Error('Tag does not resolve to the checked-out release commit.');
}
if (!/^[0-9a-f]{40}$/u.test(releaseCommit)) {
  throw new Error('Resolved release commit is invalid.');
}
if (process.env.GITHUB_OUTPUT !== undefined) {
  appendFileSync(
    process.env.GITHUB_OUTPUT,
    `tag=${tag}\nversion=${expected}\ncommit=${releaseCommit}\nnotes_path=${notesPath}\nrepository=${releaseConfig.repository}\n`,
    'utf8',
  );
}
console.log(
  `Verified ${dryRun ? 'dry-run' : 'annotated'} release identity ${tag} at ${releaseCommit} for ${releaseConfig.repository}.`,
);

function git(args, includeStderr = false) {
  const result = spawnSync('git', args, { cwd: repositoryRoot, encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`Release tag ${tag} is unavailable or invalid.`);
  return `${result.stdout}${includeStderr ? result.stderr : ''}`;
}
