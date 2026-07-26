import type { BrowserWindow } from 'electron';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { PiInstallationService } from '../../app/src/main/providers/pi-installation-service';

afterEach(() => vi.useRealTimers());

describe('Pi installation service', () => {
  it('keeps unbounded dialog dwell separate from validation and persistence', async () => {
    vi.useFakeTimers();
    let resolveDialog: ((path: string | null) => void) | undefined;
    const dialogSelection = new Promise<string | null>((resolve) => {
      resolveDialog = resolve;
    });
    const update = vi.fn();
    const settings = {
      get: () => ({ smartProcessing: { piInstallationPath: null } }),
      update,
    } as unknown as SettingsStore;
    const service = new PiInstallationService(settings, {
      platform: 'linux',
      environment: {},
      dialogs: { choose: () => dialogSelection },
    });
    const browsing = service.browse({} as BrowserWindow);

    await vi.advanceTimersByTimeAsync(10 * 60_000);
    resolveDialog?.('/chosen/pi');

    await expect(browsing).resolves.toBe('/chosen/pi');
    expect(update).not.toHaveBeenCalled();
  });

  it('does not persist a path when the pre-commit status probe times out', async () => {
    vi.useFakeTimers();
    const update = vi.fn();
    const settings = {
      get: () => ({ smartProcessing: { piInstallationPath: null } }),
      update,
    } as unknown as SettingsStore;
    const service = new PiInstallationService(settings, {
      platform: 'linux',
      environment: {},
      statusProbe: (_path, signal) =>
        new Promise((_resolve, reject) => {
          signal.addEventListener('abort', () => reject(new DOMException('Aborted', 'AbortError')));
        }),
    });

    const saving = service.save(null);
    const rejected = expect(saving).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.advanceTimersByTimeAsync(120_000);

    await rejected;
    expect(update).not.toHaveBeenCalled();
  });
});
