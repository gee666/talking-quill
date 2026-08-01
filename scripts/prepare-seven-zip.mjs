import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { appendFile, mkdir, rm, writeFile } from 'node:fs/promises';
import { createRequire } from 'node:module';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const VERSION = '25.01';
const ARCHIVE_URL = 'https://www.7-zip.org/a/7z2501-extra.7z';
const ARCHIVE_SHA256 = 'cd3cf38085c2cc6839cf72716dafb3175ae425f4fd34faafc6c0b64d618d307f';
const require = createRequire(import.meta.url);
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const architecture = process.argv[2];

if (process.platform !== 'win32') throw new Error('Pinned 7-Zip preparation requires Windows.');
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Expected 7-Zip architecture: x64 or arm64.');
}

const toolRoot = resolve(repositoryRoot, 'tmp', 'release-tools', `7zip-${VERSION}`);
const archivePath = resolve(toolRoot, '7zip-extra.7z');
await rm(toolRoot, { recursive: true, force: true });
await mkdir(toolRoot, { recursive: true });
const archive = await downloadWithRetry(ARCHIVE_URL);
const digest = createHash('sha256').update(archive).digest('hex');
if (digest !== ARCHIVE_SHA256) {
  throw new Error(`Pinned 7-Zip archive checksum mismatch: ${digest}`);
}
await writeFile(archivePath, archive, { flag: 'wx' });

const bootstrap = bundledX64SevenZip();
const extraction = spawnSync(bootstrap, ['x', '-y', `-o${toolRoot}`, archivePath], {
  encoding: 'utf8',
});
if (extraction.status !== 0) {
  throw new Error(
    `Unable to extract pinned 7-Zip: ${extraction.error?.message ?? extraction.stderr.trim()}`,
  );
}

const executable = resolve(toolRoot, architecture, '7za.exe');
const verification = spawnSync(executable, ['i'], { encoding: 'utf8' });
if (
  verification.status !== 0 ||
  !verification.stdout.includes(
    `7-Zip (a) ${VERSION} (${architecture === 'arm64' ? 'arm64' : 'x64'})`,
  )
) {
  throw new Error(
    `Pinned 7-Zip verification failed: ${verification.error?.message ?? verification.stderr.trim()}`,
  );
}

if (process.env.GITHUB_ENV === undefined) {
  throw new Error('GITHUB_ENV is required to bind the pinned release extractor.');
}
await appendFile(process.env.GITHUB_ENV, `TALKING_QUILL_7ZIP_PATH=${executable}\n`, 'utf8');
console.log(`Prepared and verified 7-Zip ${VERSION} (${architecture}) at ${executable}`);

async function downloadWithRetry(url) {
  let detail = 'unknown error';
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      const response = await fetch(url, { redirect: 'follow' });
      if (response.ok) return Buffer.from(await response.arrayBuffer());
      detail = `${String(response.status)} ${response.statusText}`;
    } catch (error) {
      detail = error instanceof Error ? error.message : String(error);
    }
    if (attempt < 3) await new Promise((resolveDelay) => setTimeout(resolveDelay, 2_000));
  }
  throw new Error(`Unable to download pinned 7-Zip ${VERSION}: ${detail}`);
}

function bundledX64SevenZip() {
  const packageRoot = dirname(require.resolve('7zip-bin/package.json'));
  return resolve(packageRoot, 'win', 'x64', '7za.exe');
}
