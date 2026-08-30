import { spawnSync } from 'node:child_process';
import { describe, expect, it, vi } from 'vitest';
import {
  CLEANUP_TIMEOUT_MS,
  resolveWindowsPowerShell,
  runWindowsBootstrapCleanup,
  type CleanupSpawn,
} from '../../scripts/run-windows-bootstrap-cleanup.mjs';

describe('Windows bootstrap cleanup runner', () => {
  it('resolves Windows PowerShell from the system root without PATH lookup', () => {
    expect(resolveWindowsPowerShell({ sYsTeMrOoT: 'D:/Windows' }, 'win32')).toBe(
      String.raw`D:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe`,
    );
  });

  it.each([
    [{ SystemRoot: 'C:/Windows' }, 'darwin'],
    [{}, 'win32'],
    [{ SystemRoot: String.raw`\\server\Windows` }, 'win32'],
    [{ SystemRoot: 'C:/base/../Windows' }, 'win32'],
    [{ SystemRoot: 'C:/Windows', SYSTEMROOT: 'D:/Windows' }, 'win32'],
  ] as const)(
    'rejects an unsupported or ambiguous launch environment %#',
    (environment, platform) => {
      expect(() => resolveWindowsPowerShell(environment, platform)).toThrow();
    },
  );

  it('inherits output, applies a timeout, forwards arguments, and returns the child status', () => {
    const spawn = vi.fn<CleanupSpawn>(() => ({ status: 7, signal: null }));

    expect(
      runWindowsBootstrapCleanup(['-Apply', '-MinimumAgeHours', '48'], {
        environment: { SystemRoot: 'C:/Windows' },
        platform: 'win32',
        spawn,
      }),
    ).toBe(7);
    expect(spawn).toHaveBeenCalledOnce();

    const call = spawn.mock.calls[0];
    if (call === undefined) throw new Error('PowerShell was not launched');
    const [executable, arguments_, options] = call;
    expect(executable).toBe(String.raw`C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe`);
    expect(arguments_).toEqual([
      '-NoProfile',
      '-NonInteractive',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      expect.stringMatching(/cleanup-windows-protected-bootstrap-leaves\.ps1$/u),
      '-Apply',
      '-MinimumAgeHours',
      '48',
    ]);
    expect(options).toEqual({
      shell: false,
      stdio: 'inherit',
      timeout: CLEANUP_TIMEOUT_MS,
      windowsHide: true,
    });
  });

  it.runIf(process.platform !== 'win32')('enforces win32 when invoked as a CLI', () => {
    const result = spawnSync(process.execPath, ['scripts/run-windows-bootstrap-cleanup.mjs'], {
      encoding: 'utf8',
      windowsHide: true,
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('Bootstrap cleanup is supported only on Windows.');
  });

  it('propagates launch failures and rejects signal-only completion', () => {
    const launchError = new Error('launch failed');
    expect(() =>
      runWindowsBootstrapCleanup([], {
        environment: { SystemRoot: 'C:/Windows' },
        platform: 'win32',
        spawn: () => ({ error: launchError, status: null, signal: null }),
      }),
    ).toThrow(launchError);
    expect(() =>
      runWindowsBootstrapCleanup([], {
        environment: { SystemRoot: 'C:/Windows' },
        platform: 'win32',
        spawn: () => ({ status: null, signal: 'SIGTERM' }),
      }),
    ).toThrow('Windows PowerShell cleanup exited due to SIGTERM.');
  });
});
