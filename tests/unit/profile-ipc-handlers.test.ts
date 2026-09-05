import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';
import { shortcutFromLegacyActivation } from '../../app/src/shared/schemas/shortcut';

const context = {
  webContentsId: 1,
  onDestroyed: () => () => undefined,
};

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe('profile IPC handlers', () => {
  it.each(['throw', 'reject'] as const)(
    'continues settings and profile mutations after a profile %s',
    async (failureMode) => {
      const settings = structuredClone(DEFAULT_SETTINGS);
      const failure = new Error('profile write failed');
      const calls: string[] = [];
      const handlers = createHandlers({
        echo: {
          resetProfile: () => {
            calls.push('reset');
            if (failureMode === 'throw') throw failure;
            return Promise.reject(failure);
          },
          deleteProfile: () => {
            calls.push('delete');
            return Promise.resolve(settings);
          },
        },
        state: {
          getSettings: () => settings,
          updateSettings: () => {
            calls.push('settings');
            return Promise.resolve(settings);
          },
        },
      } as unknown as HandlerDependencies);

      const failed = handlers['profile:reset']({ id: 'general' }, context);
      const updated = handlers['settings:update']({ app: { closeToTray: true } }, context);
      const deleted = handlers['profile:delete']({ id: 'custom' }, context);

      await expect(failed).rejects.toBe(failure);
      await Promise.all([updated, deleted]);
      expect(calls).toEqual(['reset', 'settings', 'delete']);
    },
  );

  it('queues profile imports behind settings but checks the dialog owner immediately', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const writing = deferred<Settings>();
    const owner = { id: context.webContentsId };
    const getByWebContentsId = vi.fn(() => owner);
    const importDictationProfiles = vi.fn(() => Promise.resolve({ status: 'cancelled' }));
    const handlers = createHandlers({
      state: { getSettings: () => settings, updateSettings: () => writing.promise },
      windows: { getByWebContentsId },
      settingsTransferFiles: { importDictationProfiles },
    } as unknown as HandlerDependencies);

    const write = handlers['settings:update']({ app: { closeToTray: true } }, context);
    const imported = handlers['profile:import-file']({}, context);
    expect(getByWebContentsId).toHaveBeenCalledWith(context.webContentsId);
    await Promise.resolve();
    expect(importDictationProfiles).not.toHaveBeenCalled();

    writing.resolve(settings);
    await Promise.all([write, imported]);
    expect(importDictationProfiles).toHaveBeenCalledExactlyOnceWith(owner);
  });

  it('does not share a mutation queue between independently created handler maps', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const writing = deferred<Settings>();
    const first = createHandlers({
      echo: { resetProfile: () => writing.promise },
    } as unknown as HandlerDependencies);
    const resetProfile = vi.fn(() => Promise.resolve(settings));
    const second = createHandlers({ echo: { resetProfile } } as unknown as HandlerDependencies);

    const pending = first['profile:reset']({ id: 'general' }, context);
    await second['profile:reset']({ id: 'general' }, context);
    expect(resetProfile).toHaveBeenCalledOnce();
    writing.resolve(settings);
    await pending;
  });

  it('routes profile mutations without Welcome activation prerequisites', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const echo = {
      createProfile: vi.fn(() => Promise.resolve(settings)),
      updateProfile: vi.fn(() => Promise.resolve(settings)),
      deleteProfile: vi.fn(() => Promise.resolve(settings)),
      resetProfile: vi.fn(() => Promise.resolve(settings)),
    };
    const handlers = createHandlers({ echo, welcome: {} } as unknown as HandlerDependencies);

    await handlers['profile:create'](
      {
        name: 'Custom',
        shortcut: shortcutFromLegacyActivation('Q', false),
        processingMode: 'raw',
        smartPrompt: null,
      },
      context,
    );
    await handlers['profile:update'](
      { id: 'general', patch: { name: 'Renamed General' } },
      context,
    );
    await handlers['profile:delete']({ id: '11111111-1111-4111-8111-111111111111' }, context);
    await handlers['profile:reset']({ id: 'prompt' }, context);

    expect(echo.createProfile).toHaveBeenCalledOnce();
    expect(echo.updateProfile).toHaveBeenCalledOnce();
    expect(echo.deleteProfile).toHaveBeenCalledOnce();
    expect(echo.resetProfile).toHaveBeenCalledOnce();
  });

  it('shares one handler mutation queue with activation-affecting settings writes', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const creating = deferred<Settings>();
    const createProfile = vi.fn(() => creating.promise);
    const updateGeneral = vi.fn(() => Promise.resolve(settings));
    const handlers = createHandlers({
      echo: { createProfile, updateGeneral },
      state: {
        getSettings: () => settings,
        getState: () => ({ enabled: settings.app.enabled }),
      },
      welcome: {},
    } as unknown as HandlerDependencies);

    const create = handlers['profile:create'](
      {
        name: 'Queued profile',
        shortcut: shortcutFromLegacyActivation('Q', false),
        processingMode: 'raw',
        smartPrompt: null,
      },
      context,
    );
    const disable = handlers['app:set-enabled']({ enabled: false }, context);
    await vi.waitFor(() => expect(createProfile).toHaveBeenCalledOnce());
    expect(updateGeneral).not.toHaveBeenCalled();

    creating.resolve(settings);
    await create;
    await disable;
    expect(updateGeneral).toHaveBeenCalledWith({ app: { enabled: false } });
  });
});
