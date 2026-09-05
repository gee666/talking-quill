import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseReleaseTag, releaseConfig } from './release-config.mjs';

export function validatePublicRelease(release, latest, { tag, commit, assetNames }) {
  const expectedUrl = `https://github.com/${releaseConfig.repository}/releases/tag/${tag}`;
  const validate = (value, label) => {
    const names = value.assets?.map((asset) => asset.name).sort();
    if (
      value.draft !== false ||
      value.prerelease !== false ||
      value.tag_name !== tag ||
      value.target_commitish !== commit ||
      value.html_url !== expectedUrl ||
      JSON.stringify(names) !== JSON.stringify([...assetNames].sort())
    ) {
      throw new Error(`${label} does not expose the exact stable release identity and assets.`);
    }
  };
  validate(release, 'Published tag response');
  validate(latest, 'Public latest response');
}

function main() {
  const [rawTag, commit, manifestInput, releaseInput, latestInput] = process.argv
    .slice(2)
    .filter((value) => value !== '--');
  if (![rawTag, commit, manifestInput, releaseInput, latestInput].every(Boolean)) {
    throw new Error(
      'Usage: verify-public-release <tag> <commit> <manifest> <tag-response> <latest-response>.',
    );
  }
  const { tag } = parseReleaseTag(rawTag);
  if (!/^[0-9a-f]{40}$/u.test(commit)) throw new Error('Published release commit is invalid.');
  const manifest = JSON.parse(readFileSync(resolve(manifestInput), 'utf8'));
  if (
    manifest.repository !== releaseConfig.repository ||
    manifest.tag !== tag ||
    manifest.sourceCommit !== commit ||
    !Array.isArray(manifest.assets)
  ) {
    throw new Error('Published release manifest identity is invalid.');
  }
  const assetNames = [
    ...manifest.assets.map((asset) => asset.name),
    'release-manifest.json',
    'SHA256SUMS.txt',
  ];
  validatePublicRelease(
    JSON.parse(readFileSync(resolve(releaseInput), 'utf8')),
    JSON.parse(readFileSync(resolve(latestInput), 'utf8')),
    { tag, commit, assetNames },
  );
  console.log(
    `Public latest release verified ${tag} at ${commit} with ${assetNames.length} assets.`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
