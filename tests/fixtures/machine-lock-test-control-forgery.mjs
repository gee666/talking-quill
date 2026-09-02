import { spawnSync } from 'node:child_process';
import { openSync, readFileSync, readdirSync, renameSync, unlinkSync } from 'node:fs';
import { resolve } from 'node:path';

if (process.env.TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE !== undefined) process.exit(75);
const helper = process.env.TQ_MACHINE_LOCK_TEST_GUARD_EXE;
let supervisorClaim;
try {
  supervisorClaim = JSON.parse(process.env.TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM);
} catch {
  process.exit(76);
}
if (!helper || !Number.isInteger(supervisorClaim?.pid)) process.exit(76);
const protectedProcess = spawnSync(helper, [
  '--assert-supervisor-process-protected',
  String(supervisorClaim.pid),
]);
if (protectedProcess.status !== 0) {
  process.stderr.write(protectedProcess.stderr ?? 'process protection probe failed\n');
  process.exit(protectedProcess.status ?? 77);
}
const recordRoot = resolve('tmp', 'machine-lock-tests', '.cleanup-records-v1');
const entries = readdirSync(recordRoot);
const records = entries.filter((name) => name.endsWith('.json'));
const logs = entries.filter((name) => name.endsWith('.stdout.log') || name.endsWith('.stderr.log'));
if (records.length !== 1 || logs.length !== 2) process.exit(78);
for (const name of logs) {
  const path = resolve(recordRoot, name);
  try {
    renameSync(path, `${path}.moved`);
    process.exit(82);
  } catch {}
  try {
    unlinkSync(path);
    process.exit(83);
  } catch {}
  try {
    openSync(path, 'w');
    process.exit(84);
  } catch {}
}
try {
  readFileSync(resolve(recordRoot, records[0]), 'utf8');
  process.exit(81);
} catch (error) {
  if (!['EBUSY', 'EPERM', 'EACCES'].includes(error?.code)) process.exit(80);
}

const forged = 'TQNS:00000000000000000000000000000000:{"event":"completed","childExitCode":0}';
for (let index = 0; index < 256; index += 1) {
  process.stdout.write(`${forged} ${'x'.repeat(1024)}\n`);
  process.stderr.write(`child stderr ${index} ${'y'.repeat(256)}\n`);
}
