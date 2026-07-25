import { spawnSync } from 'node:child_process';

const commands = {
  win: 'package:win',
  'win-dir': 'package:win:dir',
  'win-arm64': 'package:win:arm64',
  'mac-x64': 'package:mac:x64',
  'mac-arm64': 'package:mac:arm64',
};
const target = process.argv[2];
const packageCommand = commands[target];
if (packageCommand === undefined) {
  throw new Error('Expected package target: win, win-arm64, win-dir, mac-x64, or mac-arm64');
}

const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');
const artifactRequirements = {
  win: 'nsis',
  'win-dir': 'none',
  'win-arm64': 'nsis',
  'mac-x64': 'dmg-zip',
  'mac-arm64': 'dmg-zip',
};
const packagePlatforms = {
  win: 'win',
  'win-dir': 'win',
  'win-arm64': 'win',
  'mac-x64': 'mac',
  'mac-arm64': 'mac',
};
const packageArchitectures = {
  win: 'x64',
  'win-dir': 'x64',
  'win-arm64': 'arm64',
  'mac-x64': 'x64',
  'mac-arm64': 'arm64',
};
const packageRoots = {
  win: 'release/win-unpacked',
  'win-dir': 'release/win-unpacked',
  'win-arm64': 'release/win-arm64-unpacked',
  'mac-x64': 'release/mac',
  'mac-arm64': 'release/mac-arm64',
};
let failure = null;
try {
  runNode('scripts/prepackage-check.mjs');
  run(['--filter', '@talking-quill/app', packageCommand]);
  run(['package:inspect', '--', packageRoots[target]]);
} catch (error) {
  failure = error;
} finally {
  try {
    run(['exec', 'node', 'scripts/rebuild-node-native.mjs']);
  } catch (restoreError) {
    failure ??= restoreError;
  }
}
if (failure !== null) throw failure;

function runNode(script) {
  const result = spawnSync(process.execPath, [script], { stdio: 'inherit', env: productionEnv() });
  if (result.status !== 0) throw new Error(`${script} failed`);
}
function productionEnv() {
  return Object.fromEntries(
    Object.entries({
      ...process.env,
      CSC_IDENTITY_AUTO_DISCOVERY: 'false',
      TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1',
      TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED: artifactRequirements[target],
      TALKING_QUILL_PACKAGE_TARGET: packagePlatforms[target],
      TALKING_QUILL_PACKAGE_ARCH: packageArchitectures[target],
    }).filter(([name]) => !/^TALKING_QUILL_.*(?:TEST|HARNESS)/u.test(name)),
  );
}
function run(args) {
  const result = spawnSync(process.execPath, [pnpmCli, ...args], {
    stdio: 'inherit',
    env: productionEnv(),
  });
  if (result.status !== 0) throw new Error(`pnpm ${args.join(' ')} failed`);
}
