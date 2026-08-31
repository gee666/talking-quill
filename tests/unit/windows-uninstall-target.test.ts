import { describe, expect, it } from 'vitest';
import { resolveSignedInWindowsUserDataTarget } from '../../app/src/main/app/windows-uninstall-target';

describe('signed-in Windows uninstall target', () => {
  it('derives the owned tree directly from Electron roaming AppData without an interpreter', () => {
    expect(
      resolveSignedInWindowsUserDataTarget(
        String.raw`C:\Users\Profile With Spaces 用户\AppData\Roaming`,
      ),
    ).toBe(String.raw`C:\Users\Profile With Spaces 用户\AppData\Roaming\Talking Quill`);
  });

  it('fails closed for an empty or NUL-bearing AppData root', () => {
    expect(() => resolveSignedInWindowsUserDataTarget('')).toThrow(
      'Windows roaming AppData root is unavailable',
    );
    expect(() => resolveSignedInWindowsUserDataTarget('C:\\bad\0root')).toThrow(
      'Windows roaming AppData root is unavailable',
    );
  });
});
