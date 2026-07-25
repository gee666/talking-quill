import { rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { spawnSync } from 'node:child_process';

if (process.platform !== 'win32') {
  throw new Error('Instrumented packaged fake-media evidence currently requires Windows');
}
const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');

const output = resolve('tmp', 'packaged-media-build');
rmSync(output, { recursive: true, force: true });
const buildEnvironment = { ...process.env, TALKING_QUILL_TASK6_TEST_HARNESS: '1' };
let failure = null;
try {
  run(['--filter', '@talking-quill/app', 'build:test'], buildEnvironment);
  run(['exec', 'node', 'scripts/build-helper.mjs', '--platform', 'win32', '--arch', 'x64']);
  run([
    '--dir',
    'app',
    'exec',
    'electron-builder',
    '--config',
    '../build/electron-builder.packaged-test.yml',
    '--dir',
    '--win',
    '--x64',
  ]);
  run(
    [
      'exec',
      'playwright',
      'test',
      'tests/e2e/packaged.spec.ts',
      '--project=packaged',
      '--grep',
      'packaged fake media',
    ],
    {
      ...process.env,
      TALKING_QUILL_PACKAGED_MEDIA_HARNESS: '1',
      TALKING_QUILL_PACKAGE_ROOT: resolve(output, 'win-unpacked'),
      TALKING_QUILL_PACKAGE_EXECUTABLE: 'Talking Quill Packaged Test.exe',
    },
  );
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

console.log(
  'Instrumented packaged-test composition passed fake media through production capture/worklet/session/widget code. This is not a release artifact.',
);

function run(args, environment = process.env) {
  const result = spawnSync(process.execPath, [pnpmCli, ...args], {
    cwd: process.cwd(),
    env: environment,
    stdio: 'inherit',
  });
  if (result.status !== 0) throw new Error(`pnpm ${args.join(' ')} failed`);
}
