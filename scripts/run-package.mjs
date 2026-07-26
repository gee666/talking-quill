import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const PACKAGE_TARGETS = Object.freeze({
  win: Object.freeze({
    command: 'package:win',
    artifactRequirement: 'nsis',
    platform: 'win',
    architecture: 'x64',
  }),
  'win-dir': Object.freeze({
    command: 'package:win:dir',
    artifactRequirement: 'none',
    platform: 'win',
    architecture: 'x64',
  }),
  'win-arm64': Object.freeze({
    command: 'package:win:arm64',
    artifactRequirement: 'nsis',
    platform: 'win',
    architecture: 'arm64',
  }),
  'mac-x64': Object.freeze({
    command: 'package:mac:x64',
    artifactRequirement: 'dmg-zip',
    platform: 'mac',
    architecture: 'x64',
  }),
  'mac-arm64': Object.freeze({
    command: 'package:mac:arm64',
    artifactRequirement: 'dmg-zip',
    platform: 'mac',
    architecture: 'arm64',
  }),
});

export function createPackagePlan(target) {
  const configuration = Object.hasOwn(PACKAGE_TARGETS, target)
    ? PACKAGE_TARGETS[target]
    : undefined;
  if (configuration === undefined) {
    throw new Error('Expected package target: win, win-arm64, win-dir, mac-x64, or mac-arm64');
  }
  return {
    ...configuration,
    pnpmArguments: ['--filter', '@talking-quill/app', configuration.command],
  };
}

function main() {
  const plan = createPackagePlan(process.argv[2]);
  const pnpmCli = process.env.npm_execpath;
  if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');
  const environment = productionEnvironment(plan);
  let failure = null;
  try {
    runPnpm(pnpmCli, plan.pnpmArguments, environment);
  } catch (error) {
    failure = error;
  } finally {
    try {
      runPnpm(pnpmCli, ['exec', 'node', 'scripts/rebuild-node-native.mjs'], environment);
    } catch (restoreError) {
      failure ??= restoreError;
    }
  }
  if (failure !== null) throw failure;
}

function productionEnvironment(plan) {
  return Object.fromEntries(
    Object.entries({
      ...process.env,
      CSC_IDENTITY_AUTO_DISCOVERY: 'false',
      TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1',
      TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: plan.artifactRequirement,
      TALKING_QUILL_PACKAGE_TARGET: plan.platform,
      TALKING_QUILL_PACKAGE_ARCH: plan.architecture,
    }).filter(([name]) => !/^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(name)),
  );
}

function runPnpm(pnpmCli, arguments_, environment) {
  const result = spawnSync(process.execPath, [pnpmCli, ...arguments_], {
    stdio: 'inherit',
    env: environment,
  });
  if (result.status !== 0) throw new Error(`pnpm ${arguments_.join(' ')} failed`);
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) main();
