import { readFile } from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(import.meta.dirname, '..');

export function createWindowsInstallerUiSmokePlan({
  architecture,
  version,
  installer,
  provenance,
  output,
  variant = 'canonical',
} = {}) {
  if (!['x64', 'arm64'].includes(architecture))
    throw new Error('Installer UI smoke requires x64 or arm64');
  if (!/^\d+\.\d+\.\d+$/u.test(version ?? ''))
    throw new Error('Installer UI smoke version is invalid');
  if (!['canonical', 'installed-acceptance'].includes(variant)) {
    throw new Error('Installer UI smoke package variant is invalid');
  }
  const outputDirectory =
    variant === 'installed-acceptance' ? 'tmp/installed-acceptance-build' : 'release';
  return Object.freeze({
    installer: resolve(
      root,
      installer ?? `${outputDirectory}/Talking-Quill-${version}-win-${architecture}.exe`,
    ),
    provenance: resolve(root, provenance ?? 'artifact-provenance.json'),
    output: resolve(root, output ?? `tmp/windows-installer-ui-smoke-${architecture}.json`),
    architecture,
  });
}

async function main() {
  if (process.platform !== 'win32') throw new Error('Installer UI smoke requires Windows');
  const app = JSON.parse(await readFile(resolve(root, 'app/package.json'), 'utf8'));
  const architecture = option('--arch') ?? process.env.TALKING_QUILL_PACKAGE_ARCH;
  const plan = createWindowsInstallerUiSmokePlan({
    architecture,
    version: app.version,
    installer: option('--installer'),
    provenance: option('--provenance'),
    output: option('--output'),
    variant: process.env.TALKING_QUILL_PACKAGE_VARIANT,
  });
  const result = spawnSync(
    'pwsh.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      resolve(root, 'scripts/windows-installer-ui-smoke.ps1'),
      '-Installer',
      plan.installer,
      '-Provenance',
      plan.provenance,
      '-Architecture',
      plan.architecture,
      '-Output',
      plan.output,
      '-TimeoutSeconds',
      '30',
    ],
    { cwd: root, stdio: 'inherit', windowsHide: true, timeout: 180_000 },
  );
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0)
    throw new Error(`Windows installer UI smoke failed with ${String(result.status)}`);
}

function option(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
