import { createHash } from 'node:crypto';
import { lstatSync, readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { parseReleaseTag, releaseConfig } from './release-config.mjs';

const args = process.argv.slice(2).filter((argument) => argument !== '--');
if (args.length !== 6)
  throw new Error(
    'Usage: verify-draft-release <tag> <commit> <manifest> <checksums> <GitHub-API-response> <downloaded-assets>.',
  );
const { tag } = parseReleaseTag(args[0]);
const commit = args[1];
if (!/^[0-9a-f]{40}$/u.test(commit)) throw new Error('Expected draft commit is invalid.');
const manifestPath = resolve(args[2]);
const directory = dirname(manifestPath);
const manifest = JSON.parse(readFileSync(manifestPath, 'utf8'));
if (
  manifest.repository !== releaseConfig.repository ||
  manifest.tag !== tag ||
  manifest.sourceCommit !== commit
) {
  throw new Error(
    'Release manifest identity does not match the expected repository, tag, and commit.',
  );
}
const checksums = parseChecksums(readFileSync(resolve(args[3]), 'utf8'));
const response = JSON.parse(readFileSync(resolve(args[4]), 'utf8'));
const downloadedDirectory = resolve(args[5]);
const expectedNames = [
  ...manifest.assets.map((asset) => asset.name),
  'release-manifest.json',
  'SHA256SUMS.txt',
].sort();
const actualAssets = response.assets?.map((asset) => asset.name).sort();
if (
  response.draft !== true ||
  response.prerelease !== false ||
  response.tag_name !== tag ||
  response.target_commitish !== commit ||
  !isAuthenticatedDraftUrl(response.html_url) ||
  !Array.isArray(actualAssets) ||
  JSON.stringify(actualAssets) !== JSON.stringify(expectedNames)
)
  throw new Error(
    'Authenticated GitHub draft response does not match the release manifest and commit.',
  );
for (const asset of response.assets) {
  const path = resolve(directory, asset.name);
  const actualDigest = createHash('sha256').update(readFileSync(path)).digest('hex');
  const downloadedPath = resolve(downloadedDirectory, asset.name);
  const downloadedDigest = createHash('sha256').update(readFileSync(downloadedPath)).digest('hex');
  const listedDigest = checksums.get(asset.name);
  if (
    (asset.name !== 'SHA256SUMS.txt' && listedDigest !== actualDigest) ||
    downloadedDigest !== actualDigest ||
    !Number.isInteger(asset.size) ||
    asset.size !== lstatSync(path).size ||
    asset.size !== lstatSync(downloadedPath).size ||
    (asset.digest !== null &&
      asset.digest !== undefined &&
      asset.digest !== `sha256:${actualDigest}`)
  )
    throw new Error(`Authenticated draft asset bytes are unverified: ${asset.name}`);
}
console.log(
  `Authenticated draft verified ${tag} at ${commit} with ${actualAssets.length} byte-bound assets.`,
);

function isAuthenticatedDraftUrl(value) {
  const prefix = `https://github.com/${releaseConfig.repository}/releases/tag/untagged-`;
  return (
    typeof value === 'string' &&
    value.startsWith(prefix) &&
    /^[0-9a-f]+$/u.test(value.slice(prefix.length))
  );
}

function parseChecksums(source) {
  const values = new Map();
  for (const line of source.trim().split(/\r?\n/u)) {
    const match = /^([0-9a-f]{64}) {2}([^\r\n]+)$/u.exec(line);
    if (match === null || values.has(match[2]))
      throw new Error('SHA256SUMS.txt is malformed or duplicated.');
    values.set(match[2], match[1]);
  }
  return values;
}
