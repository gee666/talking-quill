import { spawnSync } from 'node:child_process';
import { existsSync, readdirSync, rmSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');

const forbiddenEnvironment = Object.keys(process.env).filter(
  (name) =>
    /^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(name) &&
    process.env[name] !== '' &&
    process.env[name] !== '0',
);
if (forbiddenEnvironment.length > 0) {
  throw new Error(
    `Production packaging rejects test-harness environment: ${forbiddenEnvironment.sort().join(', ')}`,
  );
}

cleanTargetArtifacts();
run('scripts/generate-notices.mjs', ['--check']);
run('scripts/model-manifest.mjs', ['--check']);
run('scripts/nsis-uninstall-policy.mjs', []);

function cleanTargetArtifacts() {
  const release = resolve(root, 'release');
  if (!existsSync(release)) return;
  const lifecycleTarget = lifecyclePackageTarget(process.env.npm_lifecycle_event);
  const target = process.env.TALKING_QUILL_PACKAGE_TARGET ?? lifecycleTarget?.target;
  const arch = process.env.TALKING_QUILL_PACKAGE_ARCH ?? lifecycleTarget?.arch;
  if ((target !== 'win' && target !== 'mac') || (arch !== 'x64' && arch !== 'arm64')) {
    throw new Error('Packaging target identity is required before cleaning release artifacts');
  }
  const unpacked =
    target === 'win'
      ? arch === 'x64'
        ? ['win-unpacked']
        : ['win-arm64-unpacked']
      : arch === 'x64'
        ? ['mac']
        : ['mac-arm64'];
  for (const name of unpacked) rmSync(resolve(release, name), { recursive: true, force: true });
  const marker = `-${target}-${arch}.`;
  for (const name of readdirSync(release)) {
    if (name.includes(marker) || name.includes(`${marker.replace(/\.$/u, '')}.__uninstaller.`))
      rmSync(resolve(release, name), { recursive: true, force: true });
  }
}

function lifecyclePackageTarget(event) {
  switch (event) {
    case 'package:win':
    case 'package:win:dir':
      return { target: 'win', arch: 'x64' };
    case 'package:win:arm64':
      return { target: 'win', arch: 'arm64' };
    case 'package:mac:x64':
      return { target: 'mac', arch: 'x64' };
    case 'package:mac:arm64':
      return { target: 'mac', arch: 'arm64' };
    default:
      return null;
  }
}

function run(script, args) {
  const result = spawnSync(process.execPath, [script, ...args], {
    cwd: root,
    stdio: 'inherit',
    env: Object.fromEntries(
      Object.entries(process.env).filter(
        ([name]) => !/^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(name),
      ),
    ),
  });
  if (result.status !== 0) throw new Error(`${script} ${args.join(' ')} failed`);
}
