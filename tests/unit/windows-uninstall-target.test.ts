import type { SpawnSyncReturns } from 'node:child_process';
import { describe, expect, it, vi } from 'vitest';
import {
  resolveSignedInWindowsUserDataTarget,
  type TextSpawn,
} from '../../app/src/main/app/windows-uninstall-target';

function result(status: number | null, stdout = ''): SpawnSyncReturns<string> {
  return {
    pid: 1,
    output: [null, stdout, ''],
    stdout,
    stderr: '',
    status,
    signal: null,
  };
}

describe('signed-in Windows uninstall target', () => {
  it('keeps the elevated handoff Unicode-safe and binds it to the desktop session profile', () => {
    const target = String.raw`C:\Users\Profile With Spaces 用户\AppData\Roaming\Talking Quill`;
    const run = vi.fn<TextSpawn>(() => result(0, target));

    expect(resolveSignedInWindowsUserDataTarget(run, { SystemRoot: String.raw`C:\Windows` })).toBe(
      target,
    );

    const call = run.mock.calls[0];
    if (call === undefined) throw new Error('Expected resolver process call');
    const [executable, arguments_, options] = call;
    expect(executable).toBe(String.raw`C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe`);
    expect(arguments_).toContain('-NonInteractive');
    expect(options).toMatchObject({ encoding: 'utf8', windowsHide: true });
    const script = arguments_.at(-1) ?? '';
    expect(script).toContain('Where-Object SessionId -eq $sessionId');
    expect(script).toContain('ProfileList');
    expect(script).toContain('[IO.FileAttributes]::ReparsePoint');
    expect(script).toContain('$target -cne "$profile\\AppData\\Roaming\\Talking Quill"');
  });

  it('fails closed for missing, ambiguous, other-user, or reparse resolver failures', () => {
    for (const status of [1, 70, null]) {
      expect(() =>
        resolveSignedInWindowsUserDataTarget(() => result(status), {
          WINDIR: String.raw`C:\Windows`,
        }),
      ).toThrow('Could not resolve the signed-in Windows user data target');
    }
    expect(() => resolveSignedInWindowsUserDataTarget(() => result(0, ''), {})).toThrow(
      'Windows system root is unavailable',
    );
  });
});
