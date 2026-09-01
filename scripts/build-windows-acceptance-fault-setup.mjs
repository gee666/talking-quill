import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { mkdir, copyFile, readFile, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const architecture = process.argv[2];
if (!['x64', 'arm64'].includes(architecture ?? '')) {
  throw new Error('Usage: build-windows-acceptance-fault-setup.mjs <x64|arm64>');
}
if (process.env.TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD !== '1') {
  throw new Error('Acceptance-fault setup requires the installed-acceptance build gate');
}
const target = architecture === 'x64' ? 'x86_64-pc-windows-msvc' : 'aarch64-pc-windows-msvc';
const manifest = resolve(repositoryRoot, 'installer', 'windows-setup', 'Cargo.toml');
const result = spawnSync(
  'cargo',
  [
    'build',
    '--manifest-path',
    manifest,
    '--target',
    target,
    '--release',
    '--locked',
    '--features',
    'acceptance-faults',
  ],
  { cwd: repositoryRoot, stdio: 'inherit', windowsHide: true },
);
if (result.status !== 0) {
  throw new Error(`acceptance-fault Windows setup build failed for ${architecture}`);
}
const output = resolve(repositoryRoot, 'tmp', 'windows-setup-acceptance-faults', architecture);
await mkdir(output, { recursive: true });
const built = resolve(
  repositoryRoot,
  'installer',
  'windows-setup',
  'target',
  target,
  'release',
  'talking-quill-windows-setup.exe',
);
const published = resolve(output, 'talking-quill-windows-setup.exe');
await copyFile(built, published);
const setupSha256 = createHash('sha256')
  .update(await readFile(published))
  .digest('hex');
await writeFile(
  resolve(output, 'nonpromotable.json'),
  `${JSON.stringify({ schemaVersion: 1, architecture, acceptanceFaults: true, promotable: false, setupSha256 })}\n`,
  'utf8',
);
