import { readdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const tmp = resolve(root, 'tmp');
const pendingStaging = await readdir(tmp).catch(() => []);
for (const path of [
  'artifact-provenance.json',
  'app/out',
  'app/native',
  'release',
  'tmp/release-upload',
  'tmp/artifact-provenance.json.pending',
  'tmp/windows-installer-ui-smoke-x64.json',
  'tmp/windows-installer-ui-smoke-arm64.json',
  ...pendingStaging
    .filter((name) => name.startsWith('release-upload.pending-'))
    .map((name) => `tmp/${name}`),
]) {
  await rm(resolve(root, path), { recursive: true, force: true });
}
console.log('Removed disposable package outputs.');
