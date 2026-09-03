import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { currentSourceIdentity } from './source-identity.mjs';

export const CANONICAL_PACKAGE_TARGETS = Object.freeze(['win', 'win-arm64']);

const PACKAGE_TARGETS = Object.freeze({
  win: Object.freeze({
    command: 'package:win',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'x64',
  }),
  'win-arm64': Object.freeze({
    command: 'package:win:arm64',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'arm64',
  }),
  'win-dir': Object.freeze({
    command: 'package:win:dir',
    artifactRequirement: 'none',
    platform: 'win',
    architecture: 'x64',
  }),
  'win-arm64-dir': Object.freeze({
    command: 'package:win:arm64:dir',
    artifactRequirement: 'none',
    platform: 'win',
    architecture: 'arm64',
  }),
  'win-unsigned': Object.freeze({
    command: 'package:win',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'x64',
  }),
  'win-arm64-unsigned': Object.freeze({
    command: 'package:win:arm64:unsigned',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'arm64',
  }),
  'win-installed-acceptance': Object.freeze({
    command: 'package:win:installed-acceptance',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'x64',
    acceptance: true,
  }),
  'win-arm64-installed-acceptance': Object.freeze({
    command: 'package:win:arm64:installed-acceptance',
    artifactRequirement: 'native-setup',
    platform: 'win',
    architecture: 'arm64',
    acceptance: true,
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
  'mac-x64-unsigned': Object.freeze({
    command: 'package:mac:x64',
    artifactRequirement: 'dmg-zip',
    platform: 'mac',
    architecture: 'x64',
  }),
  'mac-arm64-unsigned': Object.freeze({
    command: 'package:mac:arm64',
    artifactRequirement: 'dmg-zip',
    platform: 'mac',
    architecture: 'arm64',
  }),
  'mac-owner-x64': Object.freeze({
    command: 'package:mac:owner:x64',
    artifactRequirement: 'dmg-zip',
    platform: 'mac',
    architecture: 'x64',
  }),
  'mac-owner-arm64': Object.freeze({
    command: 'package:mac:owner:arm64',
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
    throw new Error(
      'Expected package target: win, win-arm64, a matching dir target, mac-x64, mac-arm64, mac-owner-x64, mac-owner-arm64, or an unsigned variant',
    );
  }
  return {
    ...configuration,
    mode: target.endsWith('-dir') ? 'directory-test' : 'update',
    pnpmArguments: ['--filter', '@talking-quill/app', configuration.command],
  };
}

function main() {
  const plan = createPackagePlan(process.argv[2]);
  const pnpmCli = process.env.npm_execpath;
  if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');
  const environment = createProductionEnvironment(plan);
  let failure = null;
  try {
    runPnpm(pnpmCli, plan.pnpmArguments, environment);
    if (plan.platform === 'win' && plan.artifactRequirement === 'native-setup') {
      runNode('scripts/run-windows-installer-ui-smoke.mjs', environment);
    }
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

export function createProductionEnvironment(plan, sourceEnvironment = process.env) {
  const acceptance = plan.acceptance === true;
  const sourceIdentity = currentSourceIdentity({
    environment: sourceEnvironment,
    requireClean: process.env.NODE_ENV !== 'test',
  });
  return Object.fromEntries(
    Object.entries({
      ...sourceEnvironment,
      CSC_IDENTITY_AUTO_DISCOVERY: 'false',
      TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1',
      TALKING_QUILL_REQUIRE_CLEAN_SOURCE: '1',
      TALKING_QUILL_RELEASE_COMMIT: sourceIdentity.sourceCommit,
      TALKING_QUILL_RELEASE_TREE: sourceIdentity.sourceTree,
      TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: plan.artifactRequirement,
      TALKING_QUILL_PACKAGE_TARGET: plan.platform,
      TALKING_QUILL_PACKAGE_ARCH: plan.architecture,
      TALKING_QUILL_PACKAGE_VARIANT: acceptance ? 'installed-acceptance' : 'canonical',
      TALKING_QUILL_PACKAGE_MODE:
        sourceEnvironment.TALKING_QUILL_PERSONAL_FRESH_INSTALL === '1' ? 'fresh' : plan.mode,
      ...(acceptance
        ? {
            TALKING_QUILL_ACCEPTANCE_BUILD: '1',
            TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD: '1',
          }
        : {}),
    }).filter(
      ([name]) =>
        !/^TALKING_QUILL_.*(?:TEST|HARNESS|FIXTURE)/u.test(name) &&
        !/^TALKING_QUILL_.*(?:PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u.test(name) &&
        (acceptance || !/^TALKING_QUILL_.*ACCEPTANCE/u.test(name)),
    ),
  );
}

function runNode(script, environment) {
  const result = spawnSync(process.execPath, [script], {
    cwd: resolve(fileURLToPath(new URL('..', import.meta.url))),
    stdio: 'inherit',
    env: environment,
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`${script} failed`);
}

function runPnpm(pnpmCli, arguments_, environment) {
  const result = spawnSync(process.execPath, [pnpmCli, ...arguments_], {
    stdio: 'inherit',
    env: environment,
  });
  if (result.status !== 0) throw new Error(`pnpm ${arguments_.join(' ')} failed`);
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) main();
