import { spawnSync } from 'node:child_process';
import { readFileSync, readdirSync } from 'node:fs';
import { resolve } from 'node:path';

if (process.env.TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE !== undefined) process.exit(75);
const helper = process.env.TQ_MACHINE_LOCK_TEST_GUARD_EXE;
const supervisorPid = process.env.TQ_MACHINE_LOCK_TEST_SUPERVISOR_PID;
if (!helper || !supervisorPid) process.exit(76);
const protectedProcess = spawnSync(helper, [
  '--assert-supervisor-process-protected',
  supervisorPid,
]);
if (protectedProcess.status !== 0) {
  process.stderr.write(protectedProcess.stderr ?? 'process protection probe failed\n');
  process.exit(protectedProcess.status ?? 77);
}
const recordRoot = resolve('tmp', 'machine-lock-tests', '.cleanup-records-v1');
const records = readdirSync(recordRoot).filter((name) => name.endsWith('.json'));
if (records.length !== 1) process.exit(78);
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
