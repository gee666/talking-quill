import { spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, mkdirSync, renameSync, rmdirSync } from 'node:fs';
import { resolve } from 'node:path';

const namespaceId = process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID;
const guard = process.env.TQ_MACHINE_LOCK_TEST_GUARD_EXE;
if (!/^[0-9a-f]{32}$/u.test(namespaceId ?? '') || !guard) process.exit(64);

const stateRoot = resolve('tmp', 'machine-lock-tests');
const parent = resolve(stateRoot, 'helper');
const root = resolve(parent, namespaceId);
const token = randomBytes(8).toString('hex');
const movedParent = `${parent}-rename-attempt`;
const parentReplacement = resolve(stateRoot, `.parent-replacement-${token}`);

if (!renameIsBlocked(parent, movedParent)) fail('parent rename was not blocked');
mkdirSync(parentReplacement);
const parentBlocked = forceReplacement(parentReplacement, parent);
if (existsSync(parentReplacement)) rmdirSync(parentReplacement);
if (!parentBlocked) fail('parent replacement was not blocked');
if (!existsSync(root) || !existsSync(parent)) fail('retained namespace path disappeared');
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
