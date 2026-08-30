import { describe, expect, it, vi } from 'vitest';
import {
  LaunchAtLoginService,
  WINDOWS_LOGIN_START_ARGUMENT,
  classifyWindowsLoginStartArguments,
  clearLaunchAtLoginForUninstall,
} from '../../app/src/main/app/launch-at-login-service';

describe('LaunchAtLoginService', () => {
  it('reconciles restart state and confirms toggles with the OS', () => {
    let registered = false;
    const setLoginItemSettings = vi.fn(({ openAtLogin }: { openAtLogin: boolean }) => {
      registered = openAtLogin;
    });
    const service = new LaunchAtLoginService({
      getLoginItemSettings: () => ({ openAtLogin: registered }),
      setLoginItemSettings,
    });
    expect(service.reconcile(true)).toBe(true);
    service.set(false);
    expect(registered).toBe(false);
    expect(setLoginItemSettings).toHaveBeenCalledTimes(2);
  });

  it('registers the explicit validated background-start argument on Windows only', () => {
    let registered = false;
    const setLoginItemSettings = vi.fn(
      ({ openAtLogin }: { readonly openAtLogin: boolean; readonly args?: readonly string[] }) => {
        registered = openAtLogin;
      },
    );
    const windows = new LaunchAtLoginService(
      {
        getLoginItemSettings: () => ({ openAtLogin: registered }),
        setLoginItemSettings,
      },
      'win32',
    );
    registered = true;
    expect(windows.reconcile(true)).toBe(true);
    expect(setLoginItemSettings).toHaveBeenLastCalledWith({
      openAtLogin: true,
      args: [WINDOWS_LOGIN_START_ARGUMENT],
    });

    registered = false;
    const macos = new LaunchAtLoginService(
      {
        getLoginItemSettings: () => ({ openAtLogin: registered }),
        setLoginItemSettings,
      },
      'darwin',
    );
    macos.set(true);
    expect(setLoginItemSettings).toHaveBeenLastCalledWith({ openAtLogin: true });
  });

  it('confirms the Windows registration with the same argument-sensitive lookup', () => {
    let registeredArguments: readonly string[] = [];
    const queriedArguments: (readonly string[] | undefined)[] = [];
    const service = new LaunchAtLoginService(
      {
        getLoginItemSettings: (settings) => {
          queriedArguments.push(settings?.args);
          return {
            openAtLogin:
              registeredArguments.length === 1 &&
              registeredArguments[0] === WINDOWS_LOGIN_START_ARGUMENT &&
              settings?.args?.length === 1 &&
              settings.args[0] === WINDOWS_LOGIN_START_ARGUMENT,
          };
        },
        setLoginItemSettings: ({ openAtLogin, args }) => {
          registeredArguments = openAtLogin ? (args ?? []) : [];
        },
      },
      'win32',
    );

    expect(service.reconcile(true)).toBe(true);
    expect(queriedArguments).toEqual([
      [WINDOWS_LOGIN_START_ARGUMENT],
      [WINDOWS_LOGIN_START_ARGUMENT],
    ]);
  });

  it('classifies only one packaged Windows marker as a background login start', () => {
    expect(classifyWindowsLoginStartArguments([WINDOWS_LOGIN_START_ARGUMENT], true, 'win32')).toBe(
      'login-start',
    );
    expect(classifyWindowsLoginStartArguments([], true, 'win32')).toBe('absent');
    for (const arguments_ of [
      [WINDOWS_LOGIN_START_ARGUMENT, WINDOWS_LOGIN_START_ARGUMENT],
      [`${WINDOWS_LOGIN_START_ARGUMENT}=unexpected`],
      [WINDOWS_LOGIN_START_ARGUMENT, `${WINDOWS_LOGIN_START_ARGUMENT}=unexpected`],
      [WINDOWS_LOGIN_START_ARGUMENT, '--talking-quill-request-machine-quit'],
    ]) {
      expect(classifyWindowsLoginStartArguments(arguments_, true, 'win32')).toBe('invalid');
    }
    expect(classifyWindowsLoginStartArguments([WINDOWS_LOGIN_START_ARGUMENT], false, 'win32')).toBe(
      'invalid',
    );
    expect(classifyWindowsLoginStartArguments([WINDOWS_LOGIN_START_ARGUMENT], true, 'darwin')).toBe(
      'invalid',
    );
  });

  it('clears and confirms OS registration before verified uninstall reset', () => {
    let registered = true;
    const adapter = {
      getLoginItemSettings: () => ({ openAtLogin: registered }),
      setLoginItemSettings: ({ openAtLogin }: { openAtLogin: boolean }) => {
        registered = openAtLogin;
      },
    };
    clearLaunchAtLoginForUninstall(adapter);
    expect(registered).toBe(false);
    expect(() =>
      clearLaunchAtLoginForUninstall({
        getLoginItemSettings: () => ({ openAtLogin: true }),
        setLoginItemSettings: vi.fn(),
      }),
    ).toThrow('could not be registered');
  });

  it('reports registration failure and rejects work after disposal', () => {
    const service = new LaunchAtLoginService({
      getLoginItemSettings: () => ({ openAtLogin: false }),
      setLoginItemSettings: vi.fn(),
    });
    expect(() => service.set(true)).toThrow('could not be registered');
    service.dispose();
    expect(() => service.reconcile(false)).toThrow('unavailable');
  });
});
