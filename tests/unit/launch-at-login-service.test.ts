import { describe, expect, it, vi } from 'vitest';
import {
  LaunchAtLoginService,
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
