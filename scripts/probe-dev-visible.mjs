import { randomBytes } from 'node:crypto';
import { spawn } from 'node:child_process';
import { mkdtemp, rm, stat } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) throw new Error('pnpm CLI path is unavailable');
const nonce = randomBytes(16).toString('hex');
const profile = await mkdtemp(join(tmpdir(), 'talking-quill-dev-visible-'));
const marker = `TALKING_QUILL_DEV_VISIBLE:${nonce}:`;
let output = '';
let errorOutput = '';
let visiblePid;
let child;
try {
  child = spawn(process.execPath, [pnpmCli, 'dev'], {
    cwd: root,
    env: {
      ...process.env,
      NODE_ENV: 'development',
      TALKING_QUILL_DEV_VISIBLE_NONCE: nonce,
      TALKING_QUILL_DEV_VISIBLE_PROFILE: profile,
    },
    stdio: ['ignore', 'pipe', 'pipe'],
    windowsHide: false,
  });
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    process.stdout.write(chunk);
    output = bounded(output + chunk);
    const match = new RegExp(`${marker}(\\d+)`, 'u').exec(output);
    if (match?.[1] !== undefined) visiblePid = Number(match[1]);
  });
  child.stderr.on('data', (chunk) => {
    process.stderr.write(chunk);
    errorOutput = bounded(errorOutput + chunk);
  });
  const result = await boundedExit(child, 180_000);
  if (result.code !== 0 || visiblePid === undefined)
    throw new Error(
      `Development visible-window probe failed (${String(result.code)}): ${errorOutput}`,
    );
  await waitForPidGone(visiblePid, 10_000);
} finally {
  if (child !== undefined && child.exitCode === null && child.signalCode === null)
    await terminateTree(child.pid);
  await rm(profile, { recursive: true, force: true });
}
try {
  await stat(profile);
  throw new Error('Development probe profile residue remains');
} catch (error) {
  if (error?.code !== 'ENOENT') throw error;
}
console.log(
  'Bounded development probe observed a visible main BrowserWindow and left no PID/profile residue',
);

function boundedExit(processChild, timeout) {
  return new Promise((resolveExit, reject) => {
    const timer = setTimeout(() => {
      void terminateTree(processChild.pid).finally(() =>
        reject(new Error('Development visible-window probe timed out')),
      );
    }, timeout);
    processChild.once('error', (error) => {
      clearTimeout(timer);
      reject(error);
    });
    processChild.once('close', (code, signal) => {
      clearTimeout(timer);
      resolveExit({ code, signal });
    });
  });
}
async function terminateTree(pid) {
  if (!Number.isSafeInteger(pid) || pid <= 0) return;
  if (process.platform === 'win32') {
    const killer = spawn(
      resolve(process.env.SystemRoot ?? 'C:\\Windows', 'System32/taskkill.exe'),
      ['/pid', String(pid), '/t', '/f'],
      { stdio: 'ignore', windowsHide: true },
    );
    await new Promise((resolveExit) => killer.once('close', resolveExit));
  } else {
    try {
      process.kill(-pid, 'SIGKILL');
    } catch {
      try {
        process.kill(pid, 'SIGKILL');
      } catch {
        // It exited between the group and root termination attempts.
      }
    }
  }
}
async function waitForPidGone(pid, timeout) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    try {
      process.kill(pid, 0);
    } catch {
      return;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 100));
  }
  throw new Error(`Visible development process ${String(pid)} remained alive`);
}
function bounded(value) {
  return value.slice(-65_536);
}
