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
