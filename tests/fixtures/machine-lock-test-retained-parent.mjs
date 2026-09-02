import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, renameSync, rmdirSync } from 'node:fs';
import { resolve } from 'node:path';

const namespaceId = process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID;
const guard = process.env.TQ_MACHINE_LOCK_TEST_GUARD_EXE;
if (!/^[0-9a-f]{32}$/u.test(namespaceId ?? '') || !guard) process.exit(64);

const stateRoot = resolve('tmp', 'machine-lock-tests');
for (const kind of ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit']) {
  const parent = resolve(stateRoot, kind);
  const root = resolve(parent, namespaceId);
  const moved = `${root}-rename-attempt`;
  const replacement = resolve(parent, `.root-replacement-${randomBytes(8).toString('hex')}`);
  if (!renameIsBlocked(root, moved)) fail(`${kind} outer-root rename was not blocked`);
  try {
    rmdirSync(root);
    fail(`${kind} outer-root deletion was not blocked`);
  } catch {
    if (!existsSync(root)) fail(`${kind} outer root disappeared`);
  }
  mkdirSync(replacement);
  if (!forceReplacement(replacement, root)) fail(`${kind} outer-root replacement was not blocked`);
  if (existsSync(replacement)) rmdirSync(replacement);
}

const parent = resolve(stateRoot, 'helper');
const movedParent = `${parent}-rename-attempt`;
const parentReplacement = resolve(
  stateRoot,
  `.parent-replacement-${randomBytes(8).toString('hex')}`,
);
if (!renameIsBlocked(parent, movedParent)) fail('outer parent rename was not blocked');
mkdirSync(parentReplacement);
if (!forceReplacement(parentReplacement, parent)) fail('outer parent replacement was not blocked');
if (existsSync(parentReplacement)) rmdirSync(parentReplacement);
process.exit(0);

function renameIsBlocked(source, destination) {
  try {
    renameSync(source, destination);
  } catch {
    return !existsSync(destination);
  }
  try {
    renameSync(destination, source);
  } catch {
    process.exit(2);
  }
  return false;
}

function forceReplacement(source, target) {
  const result = spawnSync(guard, ['--force-directory-replacement', source, target], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) console.error(result.stderr);
  return result.status === 0;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
