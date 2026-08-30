import { randomUUID } from 'node:crypto';
import { readFile, readdir, rename, rm, mkdir, writeFile } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { join } from 'node:path';

import { verifyStagedNativeRoleSet } from './helper-build-contract.mjs';

export async function replaceNativeRoleDirectory({
  appDirectory,
  stagingDirectory,
  platform,
  architecture,
  pid = process.pid,
  processAlive = defaultProcessAlive,
}) {
  const destination = join(appDirectory, 'native');
  const backup = join(appDirectory, `.native-backup-${String(pid)}`);
  const lock = join(appDirectory, '.native-build-lock');
  const lockToken = await acquireLock({
    appDirectory,
    destination,
    lock,
    platform,
    pid,
    processAlive,
  });
  let hadPrevious = false;
  try {
    await assertLockOwnership(lock, lockToken);
    if (existsSync(backup)) {
      throw new Error(`Refusing to delete unexpected native staging backup: ${backup}`);
    }
    hadPrevious = existsSync(destination);
    if (hadPrevious) {
      // app/native can contain a developer's uncommitted binaries or notes.
      // Replace it only when the complete existing directory is itself one
      // coherent role set for this target.
      await verifyExistingNativeRoleSet(destination, platform);
      await assertLockOwnership(lock, lockToken);
      await rename(destination, backup);
    }
    try {
      await assertLockOwnership(lock, lockToken);
      await rename(stagingDirectory, destination);
    } catch (error) {
      if (hadPrevious && !existsSync(destination)) {
        await assertLockOwnership(lock, lockToken);
        await rename(backup, destination);
      }
      throw error;
    }
    try {
      await verifyStagedNativeRoleSet(destination, { platform, architecture });
    } catch (error) {
      await assertLockOwnership(lock, lockToken);
      await rm(destination, { recursive: true, force: true });
      if (hadPrevious) await rename(backup, destination);
      throw error;
    }
    await rm(backup, { recursive: true, force: true });
  } catch (error) {
    if (!existsSync(destination) && existsSync(backup)) {
      await assertLockOwnership(lock, lockToken);
      await rename(backup, destination);
    }
    throw error;
  } finally {
    await releaseLock(lock, lockToken);
  }
}

async function acquireLock({ appDirectory, destination, lock, platform, pid, processAlive }) {
  try {
    return await createLock(lock, pid);
  } catch (error) {
    if (!existsSync(lock)) throw error;
  }
  let ownerPid = null;
  try {
    const owner = JSON.parse(await readFile(join(lock, 'owner.json'), 'utf8'));
    if (Number.isSafeInteger(owner.pid) && owner.pid > 0) ownerPid = owner.pid;
  } catch {
    // A missing/truncated owner record is stale because creation writes it synchronously.
  }
  if (ownerPid !== null && processAlive(ownerPid)) {
    throw new Error(`Native role staging is already owned by process ${String(ownerPid)}`);
  }
  const quarantine = `${lock}.stale-${String(pid)}-${randomUUID()}`;
  try {
    await rename(lock, quarantine);
  } catch (error) {
    throw new Error('Another native role staging operation won stale-lock recovery', {
      cause: error,
    });
  }
  let token = null;
  try {
    token = await createLock(lock, pid);
    await assertLockOwnership(lock, token);
    await recoverMissingDestination(appDirectory, destination, platform);
    await rm(quarantine, { recursive: true, force: true });
    return token;
  } catch (error) {
    if (token !== null) await releaseLock(lock, token);
    throw error;
  }
}

async function createLock(lock, pid) {
  const token = randomUUID();
  const unpublished = `${lock}-${String(pid)}-${token}`;
  await mkdir(unpublished);
  try {
    await writeFile(
      join(unpublished, 'owner.json'),
      `${JSON.stringify({ version: 1, pid, token })}\n`,
      { flag: 'wx' },
    );
    await rename(unpublished, lock);
    return token;
  } finally {
    await rm(unpublished, { recursive: true, force: true });
  }
}

async function assertLockOwnership(lock, token) {
  const owner = JSON.parse(await readFile(join(lock, 'owner.json'), 'utf8'));
  if (owner.token !== token) throw new Error('Native staging lock ownership changed');
}

async function releaseLock(lock, token) {
  try {
    await assertLockOwnership(lock, token);
    await rm(lock, { recursive: true, force: true });
  } catch (error) {
    if (existsSync(lock)) throw error;
  }
}

async function verifyExistingNativeRoleSet(directory, platform) {
  const accepted = [];
  for (const architecture of ['x64', 'arm64']) {
    try {
      await verifyStagedNativeRoleSet(directory, { platform, architecture });
      accepted.push(architecture);
    } catch {
      // A coherent role directory has exactly one native architecture.
    }
  }
  if (accepted.length !== 1) {
    throw new Error(
      `Existing native role set mismatch: expected one coherent architecture, found ${String(accepted.length)}`,
    );
  }
}

async function recoverMissingDestination(appDirectory, destination, platform) {
  if (existsSync(destination)) return;
  const backups = (await readdir(appDirectory, { withFileTypes: true }))
    .filter((entry) => entry.isDirectory() && /^\.native-backup-[1-9][0-9]*$/u.test(entry.name))
    .map((entry) => join(appDirectory, entry.name));
  const valid = [];
  const supportedArchitectures = ['x64', 'arm64'];
  for (const backup of backups) {
    for (const architecture of supportedArchitectures) {
      try {
        await verifyStagedNativeRoleSet(backup, { platform, architecture });
        valid.push(backup);
        break;
      } catch {
        // Preserve malformed/unrelated directories; never guess that they are disposable.
      }
    }
  }
  if (valid.length !== 1) {
    throw new Error(
      `Cannot recover missing app/native: expected one verified backup, found ${String(valid.length)}`,
    );
  }
  await rename(valid[0], destination);
}

function defaultProcessAlive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code !== 'ESRCH';
  }
}
