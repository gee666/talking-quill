import { spawnSync } from 'node:child_process';
import { basename, resolve } from 'node:path';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

const root = resolve(import.meta.dirname, '..');
const architectureTest = resolve(root, 'tests/native/rust-workspace-architectures.test.mjs');
const result = spawnSync(process.execPath, ['--test', architectureTest], {
  cwd: root,
  env: sanitizedSubprocessEnvironment(),
  stdio: 'inherit',
  timeout: 21 * 60_000,
  windowsHide: true,
});
if (result.error !== undefined) throw result.error;
if (result.status !== 0) {
  throw new Error(
    `${basename(process.execPath)} --test ${architectureTest} failed with ${String(result.status)}`,
  );
}
