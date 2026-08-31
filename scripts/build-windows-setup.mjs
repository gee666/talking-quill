import { spawnSync } from 'node:child_process';
import { mkdir, copyFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: build-windows-setup.mjs <x64|arm64>');
}
const target = architecture === 'x64' ? 'x86_64-pc-windows-msvc' : 'aarch64-pc-windows-msvc';
const manifest = resolve(repositoryRoot, 'installer', 'windows-setup', 'Cargo.toml');
const result = spawnSync(
  'cargo',
  ['build', '--manifest-path', manifest, '--target', target, '--release', '--locked'],
  { cwd: repositoryRoot, stdio: 'inherit', windowsHide: true },
);
if (result.status !== 0)
  throw new Error(`native Windows bootstrap build failed for ${architecture}`);
const output = resolve(repositoryRoot, 'tmp', 'windows-setup', architecture);
await mkdir(output, { recursive: true });
await copyFile(
  resolve(
    repositoryRoot,
    'installer',
    'windows-setup',
    'target',
    target,
    'release',
    'talking-quill-windows-setup.exe',
  ),
  resolve(output, 'talking-quill-windows-setup.exe'),
);
