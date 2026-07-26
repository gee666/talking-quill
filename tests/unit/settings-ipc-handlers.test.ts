import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

const context = { webContentsId: 1, onDestroyed: () => () => undefined };

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe('settings IPC handler', () => {
  it('uses the same activation-evidence transaction for the enabled shortcut', async () => {
    let settings: Settings = structuredClone(DEFAULT_SETTINGS);
    const invalidateActivationBinding = vi.fn(() => Promise.resolve());
    const updateGeneral = vi.fn((patch: { app?: { enabled?: boolean } }) => {
      settings = { ...settings, app: { ...settings.app, ...patch.app } };
      return Promise.resolve(structuredClone(settings));
    });
    const handlers = createHandlers({
      state: {
        getSettings: () => structuredClone(settings),
        getState: () => ({ enabled: settings.app.enabled }),
      },
      echo: { updateGeneral },
      welcome: { invalidateActivationBinding },
    } as unknown as HandlerDependencies);

    await expect(handlers['app:set-enabled']({ enabled: false }, context)).resolves.toEqual({
      enabled: false,
    });
    expect(invalidateActivationBinding).toHaveBeenCalledOnce();
    expect(updateGeneral).toHaveBeenCalledWith({ app: { enabled: false } });
  });

  it('serializes launch-at-login reconciliation with the settings commit', async () => {
    let settings: Settings = structuredClone(DEFAULT_SETTINGS);
    let registered = false;
    const firstWrite = deferred();
    let updates = 0;
    const updateSettings = vi.fn(async (patch: { app?: { launchAtLogin?: boolean } }) => {
      updates += 1;
      if (updates === 1) await firstWrite.promise;
      settings = {
        ...settings,
        app: { ...settings.app, ...patch.app },
      };
      return structuredClone(settings);
    });
    const set = vi.fn((enabled: boolean) => {
      registered = enabled;
    });
    const handlers = createHandlers({
      state: {
        getSettings: () => structuredClone(settings),
        updateSettings,
      },
      launchAtLogin: { set },
      welcome: {},
    } as unknown as HandlerDependencies);

    const enable = handlers['settings:update']({ app: { launchAtLogin: true } }, context);
    await vi.waitFor(() => expect(set).toHaveBeenCalledWith(true));
    const disable = handlers['settings:update']({ app: { launchAtLogin: false } }, context);
    expect(set).toHaveBeenCalledTimes(1);

    firstWrite.resolve();
    await Promise.all([enable, disable]);

    expect(set.mock.calls).toEqual([[true], [false]]);
    expect(settings.app.launchAtLogin).toBe(false);
    expect(registered).toBe(false);
  });

  it('compensates the OS registration when the settings commit fails', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const set = vi.fn();
    const handlers = createHandlers({
      state: {
        getSettings: () => structuredClone(settings),
        updateSettings: vi.fn().mockRejectedValue(new Error('write failed')),
      },
      launchAtLogin: { set },
      welcome: {},
    } as unknown as HandlerDependencies);

    await expect(
      handlers['settings:update']({ app: { launchAtLogin: true } }, context),
    ).rejects.toThrow('write failed');
    expect(set.mock.calls).toEqual([[true], [false]]);
  });

  it('repairs a failed compensation when the persisted value is explicitly retried', async () => {
    let settings: Settings = structuredClone(DEFAULT_SETTINGS);
    let writes = 0;
    const updateSettings = vi.fn((patch: { app?: { launchAtLogin?: boolean } }) => {
      writes += 1;
      if (writes === 1) return Promise.reject(new Error('write failed'));
      settings = { ...settings, app: { ...settings.app, ...patch.app } };
      return Promise.resolve(structuredClone(settings));
    });
    let registrations = 0;
    const set = vi.fn(() => {
      registrations += 1;
      if (registrations === 2) throw new Error('compensation failed');
    });
    const handlers = createHandlers({
      state: { getSettings: () => structuredClone(settings), updateSettings },
      launchAtLogin: { set },
      welcome: {},
    } as unknown as HandlerDependencies);

    await expect(
      handlers['settings:update']({ app: { launchAtLogin: true } }, context),
    ).rejects.toThrow('write failed');
    await expect(
      handlers['settings:update']({ app: { launchAtLogin: false } }, context),
    ).resolves.toMatchObject({ app: { launchAtLogin: false } });

    expect(set.mock.calls).toEqual([[true], [false], [false]]);
  });

  it('retries evidence invalidation when the same setting patch is retried', async () => {
    let settings: Settings = structuredClone(DEFAULT_SETTINGS);
    const invalidateMicrophoneBinding = vi
      .fn<() => Promise<void>>()
      .mockRejectedValueOnce(new Error('invalidation failed'))
      .mockResolvedValue(undefined);
    const updateSettings = vi.fn((patch: { recording?: { preferredMicrophoneId?: string } }) => {
      settings = {
        ...settings,
        recording: { ...settings.recording, ...patch.recording },
      };
      return Promise.resolve(structuredClone(settings));
    });
    const handlers = createHandlers({
      state: {
        getSettings: () => structuredClone(settings),
        updateSettings,
      },
      welcome: { invalidateMicrophoneBinding },
      launchAtLogin: { set: vi.fn() },
    } as unknown as HandlerDependencies);
    const patch = { recording: { preferredMicrophoneId: 'studio-mic' } };

    await expect(handlers['settings:update'](patch, context)).rejects.toThrow(
      'invalidation failed',
    );
    await expect(handlers['settings:update'](patch, context)).resolves.toMatchObject(patch);

    expect(invalidateMicrophoneBinding).toHaveBeenCalledTimes(2);
    expect(updateSettings).toHaveBeenCalledOnce();
  });
});
