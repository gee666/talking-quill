import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, copyFile, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: build-windows-stale-schema2-cleanup-setup.mjs <x64|arm64>');
}
if (process.env.TALKING_QUILL_STALE_SCHEMA2_CLEANUP_BUILD !== '1') {
  throw new Error('Stale schema-2 cleanup setup requires its explicit build gate');
}
const target = architecture === 'x64' ? 'x86_64-pc-windows-msvc' : 'aarch64-pc-windows-msvc';
const manifest = resolve(repositoryRoot, 'installer', 'windows-setup', 'Cargo.toml');
const targetDirectory = resolve(
  repositoryRoot,
  'tmp',
  'cargo-target',
  'windows-setup-stale-schema2-cleanup',
);
const result = spawnSync(
  'cargo',
  [
    'build',
    '--manifest-path',
    manifest,
    '--target-dir',
    targetDirectory,
    '--target',
    target,
    '--release',
    '--locked',
    '--no-default-features',
    '--features',
    'stale-schema2-cleanup',
  ],
  {
    cwd: repositoryRoot,
    stdio: 'inherit',
    windowsHide: true,
    env: sanitizedSubprocessEnvironment(),
  },
);
if (result.status !== 0) {
  throw new Error(`stale schema-2 cleanup Windows setup build failed for ${architecture}`);
}
const output = resolve(repositoryRoot, 'tmp', 'windows-setup-stale-schema2-cleanup', architecture);
await mkdir(output, { recursive: true });
const built = resolve(targetDirectory, target, 'release', 'talking-quill-windows-setup.exe');
const published = resolve(output, 'talking-quill-windows-setup.exe');
await copyFile(built, published);
const publishedBytes = await readFile(published);
for (const marker of [
  'TQ_MACHINE_LOCK_TEST_NAMESPACE_ID',
  'Talking Quill Tests',
  'TalkingQuill.Tests.',
]) {
  if (
    publishedBytes.includes(Buffer.from(marker, 'ascii')) ||
    publishedBytes.includes(Buffer.from(marker, 'utf16le'))
  ) {
    throw new Error(`cleanup setup contains machine-lock test marker: ${marker}`);
  }
}
const setupSha256 = createHash('sha256').update(publishedBytes).digest('hex');
await writeFile(
  resolve(output, 'nonpromotable.json'),
  `${JSON.stringify({
    schemaVersion: 1,
    architecture,
    staleSchema2Cleanup: true,
    promotable: false,
    setupSha256,
  })}\n`,
  'utf8',
);
