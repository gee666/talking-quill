import { spawnSync } from 'node:child_process';
import { mkdir, copyFile, readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: build-windows-setup.mjs <x64|arm64>');
}
const target = architecture === 'x64' ? 'x86_64-pc-windows-msvc' : 'aarch64-pc-windows-msvc';
const manifest = resolve(repositoryRoot, 'installer', 'windows-setup', 'Cargo.toml');
const targetDirectory = resolve(repositoryRoot, 'tmp', 'cargo-target', 'windows-setup-production');
const cargoArguments = [
  'build',
  '--manifest-path',
  manifest,
  '--target-dir',
  targetDirectory,
  '--target',
  target,
  '--release',
  '--locked',
];
const result = spawnSync('cargo', cargoArguments, {
  cwd: repositoryRoot,
  stdio: 'inherit',
  windowsHide: true,
});
if (result.status !== 0)
  throw new Error(`native Windows bootstrap build failed for ${architecture}`);
const output = resolve(repositoryRoot, 'tmp', 'windows-setup', architecture);
await mkdir(output, { recursive: true });
const published = resolve(output, 'talking-quill-windows-setup.exe');
await copyFile(
  resolve(
    repositoryRoot,
    'tmp',
    'cargo-target',
    'windows-setup-production',
    target,
    'release',
    'talking-quill-windows-setup.exe',
  ),
  published,
);
const productionBytes = await readFile(published);
for (const marker of [
  '/TQ-CLEAN-STALE-SCHEMA2',
  '/TQ-DIAGNOSE-STALE-SCHEMA2',
  'TQ_MACHINE_LOCK_TEST_NAMESPACE_ID',
  'Talking Quill Tests',
  'TalkingQuill.Tests.',
]) {
  if (
    productionBytes.includes(Buffer.from(marker, 'ascii')) ||
    productionBytes.includes(Buffer.from(marker, 'utf16le'))
  ) {
    throw new Error(`canonical Windows setup contains cleanup feature marker: ${marker}`);
  }
}
