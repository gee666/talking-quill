import { existsSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { currentSourceIdentity } from './source-identity.mjs';
import { verifyCoordinatedVersions } from './release-version-policy.mjs';

const root = resolve(import.meta.dirname, '..');
const tag = process.argv.slice(2).find((value) => value !== '--') ?? process.env.RELEASE_TAG;
if (!/^v\d+\.\d+\.\d+$/u.test(tag ?? ''))
  throw new Error('Release preflight requires vMAJOR.MINOR.PATCH.');
const version = await verifyCoordinatedVersions(root);
if (tag !== `v${version}`)
  throw new Error(`Release tag ${String(tag)} does not match version ${version}.`);
const identity = currentSourceIdentity({ repositoryRoot: root, requireClean: true });
const pendingStaging = existsSync(resolve(root, 'tmp'))
  ? readdirSync(resolve(root, 'tmp'))
      .filter((name) => name.startsWith('release-upload.pending-'))
      .map((name) => `tmp/${name}`)
  : [];
const stale = [
  'artifact-provenance.json',
  'app/out',
  'app/native',
  'release',
  'tmp/release-upload',
  'tmp/artifact-provenance.json.pending',
  'tmp/windows-installer-ui-smoke-x64.json',
  'tmp/windows-installer-ui-smoke-arm64.json',
  ...pendingStaging,
].filter((path) => existsSync(resolve(root, path)));
if (stale.length > 0)
  throw new Error(`Release output must be clean before packaging: ${stale.join(', ')}`);
console.log(JSON.stringify({ tag, version, ...identity }));

if (resolve(process.argv[1] ?? '') !== fileURLToPath(import.meta.url)) {
  throw new Error('release-source-preflight.mjs is a command-only module');
}
