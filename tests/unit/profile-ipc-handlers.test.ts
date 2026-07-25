import { describe, expect, it, vi } from 'vitest';
import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';

const context = {
  role: 'main' as const,
  webContentsId: 1,
  onDestroyed: () => () => undefined,
};

describe('profile IPC handlers', () => {
  it('relies on atomic SettingsStore evidence comparison instead of unconditional invalidation', async () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    const echo = {
      createProfile: vi.fn(() => Promise.resolve(settings)),
      updateProfile: vi.fn(() => Promise.resolve(settings)),
      deleteProfile: vi.fn(() => Promise.resolve(settings)),
      resetProfile: vi.fn(() => Promise.resolve(settings)),
    };
    const invalidateActivationBinding = vi.fn(() => Promise.resolve());
    const handlers = createHandlers({
      echo,
      welcome: { invalidateActivationBinding },
    } as unknown as HandlerDependencies);

    await handlers['profile:create'](
      {
        name: 'Custom',
        activationKey: 'Q',
        shift: false,
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
    expect(invalidateActivationBinding).not.toHaveBeenCalled();
  });
});
