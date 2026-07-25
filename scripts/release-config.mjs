import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

export const repositoryRoot = resolve(import.meta.dirname, '..');
export const releaseConfig = Object.freeze(
  JSON.parse(readFileSync(resolve(repositoryRoot, 'release.config.json'), 'utf8')),
);

export function parseReleaseTag(tag) {
  const match = /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.exec(tag);
  if (match === null) throw new Error('Release tag must be strict vMAJOR.MINOR.PATCH semver.');
  return { tag, version: `${match[1]}.${match[2]}.${match[3]}` };
}

export function releaseNotesPath(version) {
  return `docs/releases/v${version}-release-notes.md`;
}
