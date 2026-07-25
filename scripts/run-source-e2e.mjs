import { spawnSync } from 'node:child_process';

const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');
let failure = null;
process.env.TALKING_QUILL_TASK6_TEST_HARNESS = '1';
process.env.TALKING_QUILL_VOCABULARY_TEST_HARNESS = '1';
process.env.TALKING_QUILL_PI_TEST_HARNESS = '1';

try {
  run(['--filter', '@talking-quill/app', 'build:test']);
  run(['test:whisper-worker']);
  run(['--filter', '@talking-quill/app', 'rebuild:native']);
  run(['exec', 'playwright', 'test', '--project=electron']);
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

function run(args) {
  const result = spawnSync(process.execPath, [pnpmCli, ...args], { stdio: 'inherit' });
  if (result.status !== 0) throw new Error(`pnpm ${args.join(' ')} failed`);
}
