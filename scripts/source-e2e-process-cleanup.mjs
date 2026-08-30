import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const profileRoot = resolve(repositoryRoot, 'tmp', 'e2e').toLowerCase();
const electronRoot = resolve(repositoryRoot, 'node_modules').toLowerCase();

export function cleanupSourceE2EProcesses() {
  if (process.platform === 'win32') return cleanupWindows();
  return cleanupPosix();
}

function cleanupWindows() {
  const query = spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "$items=@(Get-CimInstance Win32_Process | Where-Object {$_.Name -eq 'electron.exe'} | Select-Object ProcessId,CommandLine); ConvertTo-Json -Compress -InputObject $items",
    ],
    { encoding: 'utf8', windowsHide: true, timeout: 15_000 },
  );
  if (query.status !== 0) throw new Error('Could not enumerate source E2E Electron processes');
  const parsed = JSON.parse(query.stdout.trim() || '[]');
  const records = Array.isArray(parsed) ? parsed : [parsed];
  const processIds = records
    .filter((record) => {
      const commandLine = String(record?.CommandLine ?? '').toLowerCase();
      return commandLine.includes(electronRoot) && commandLine.includes(profileRoot);
    })
    .map((record) => Number(record.ProcessId))
    .filter((pid) => Number.isSafeInteger(pid) && pid > 0);
  for (const pid of processIds) {
    const killed = spawnSync('taskkill.exe', ['/PID', String(pid), '/T', '/F'], {
      encoding: 'utf8',
      windowsHide: true,
      timeout: 15_000,
    });
    if (killed.status !== 0 && !/not found|no running instance/iu.test(killed.stderr)) {
      throw new Error(`Could not stop stale source E2E Electron process ${String(pid)}`);
    }
  }
  return processIds;
}

function cleanupPosix() {
  const query = spawnSync('ps', ['-axo', 'pid=,command='], {
    encoding: 'utf8',
    timeout: 15_000,
  });
  if (query.status !== 0) throw new Error('Could not enumerate source E2E Electron processes');
  const processIds = query.stdout.split(/\r?\n/u).flatMap((line) => {
    const match = /^\s*(\d+)\s+(.*)$/u.exec(line);
    if (match?.[1] === undefined || match[2] === undefined) return [];
    const command = match[2].toLowerCase();
    return command.includes(electronRoot) && command.includes(profileRoot)
      ? [Number(match[1])]
      : [];
  });
  for (const pid of processIds) {
    try {
      process.kill(pid, 'SIGKILL');
    } catch (error) {
      if (error?.code !== 'ESRCH') throw error;
    }
  }
  return processIds;
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const stopped = cleanupSourceE2EProcesses();
  if (stopped.length > 0)
    console.log(`Stopped ${String(stopped.length)} stale source E2E process(es)`);
}
