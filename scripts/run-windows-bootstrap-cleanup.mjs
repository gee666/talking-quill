import { spawnSync } from 'node:child_process';
import { resolve, win32 } from 'node:path';
import { fileURLToPath } from 'node:url';

export const CLEANUP_TIMEOUT_MS = 15 * 60 * 1_000;
const POWERSHELL_ARGUMENTS = Object.freeze([
  '-NoProfile',
  '-NonInteractive',
  '-ExecutionPolicy',
  'Bypass',
  '-File',
  fileURLToPath(new URL('cleanup-windows-protected-bootstrap-leaves.ps1', import.meta.url)),
]);

export function resolveWindowsPowerShell(environment, platform) {
  if (platform !== 'win32') throw new Error('Bootstrap cleanup is supported only on Windows.');

  const roots = Object.entries(environment)
    .filter(([key, value]) => key.toLowerCase() === 'systemroot' && value !== undefined)
    .map(([, value]) => value);
  if (roots.length !== 1 || !/^[A-Za-z]:[\\/][^\0]*$/u.test(roots[0])) {
    throw new Error('A single absolute SystemRoot is required.');
  }

  if (roots[0].split(/[\\/]/u).includes('..')) throw new Error('SystemRoot must be canonical.');
  const systemRoot = win32.normalize(roots[0]);
  return win32.join(systemRoot, 'System32', 'WindowsPowerShell', 'v1.0', 'powershell.exe');
}

export function runWindowsBootstrapCleanup(
  forwardedArguments,
  {
    environment = process.env,
    platform = process.platform,
    spawn = spawnSync,
    timeoutMs = CLEANUP_TIMEOUT_MS,
  } = {},
) {
  const executable = resolveWindowsPowerShell(environment, platform);
  const result = spawn(executable, [...POWERSHELL_ARGUMENTS, ...forwardedArguments], {
    shell: false,
    stdio: 'inherit',
    timeout: timeoutMs,
    windowsHide: true,
  });
  if (result.error !== undefined) throw result.error;
  if (result.status === null) {
    throw new Error(
      result.signal === null
        ? 'Windows PowerShell cleanup ended without an exit code.'
        : `Windows PowerShell cleanup exited due to ${result.signal}.`,
    );
  }
  return result.status;
}

if (process.argv[1] !== undefined && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  process.exitCode = runWindowsBootstrapCleanup(process.argv.slice(2));
}
