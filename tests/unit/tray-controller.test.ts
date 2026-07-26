import { describe, expect, it, vi } from 'vitest';

const electron = vi.hoisted(() => {
  interface MenuItem {
    readonly label?: string;
    readonly enabled?: boolean;
    readonly click?: () => void;
  }
  const templates: MenuItem[][] = [];
  const destroy = vi.fn();
  class Tray {
    setToolTip = vi.fn();
    on = vi.fn();
    setContextMenu = vi.fn();
    destroy = destroy;
  }
  return {
    templates,
    destroy,
    Menu: {
      buildFromTemplate: vi.fn((template: MenuItem[]) => {
        templates.push(template);
        return template;
      }),
    },
    Tray,
    nativeImage: {
      createFromDataURL: vi.fn(() => ({
        setTemplateImage: vi.fn(),
        resize: vi.fn(() => ({})),
      })),
    },
  };
});

vi.mock('electron', () => ({
  Menu: electron.Menu,
  Tray: electron.Tray,
  nativeImage: electron.nativeImage,
}));

import { TrayController } from '../../app/src/main/app/tray-controller';
import type { AppStateService } from '../../app/src/main/app/app-state-service';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

describe('TrayController lifecycle', () => {
  it('stops new mutations, drains the active mutation, and never refreshes after destruction', async () => {
    const mutation = deferred();
    const setEnabled = vi.fn(() => mutation.promise);
    const controller = new TrayController(
      { getState: () => ({ enabled: true }) } as unknown as AppStateService,
      { showMain: vi.fn(), quit: vi.fn(), setEnabled },
    );
    const firstMenu = electron.templates.at(-1);
    const firstToggle = firstMenu?.find(({ label }) => label === 'Disable');

    firstToggle?.click?.();
    expect(setEnabled).toHaveBeenCalledOnce();
    controller.stopAccepting();
    const stoppedMenu = electron.templates.at(-1);
    const stoppedToggle = stoppedMenu?.find(({ label }) => label === 'Disable');
    expect(stoppedToggle?.enabled).toBe(false);
    stoppedToggle?.click?.();
    expect(setEnabled).toHaveBeenCalledOnce();

    let drained = false;
    const drain = controller.drain().then(() => {
      drained = true;
    });
    await Promise.resolve();
    expect(drained).toBe(false);
    const refreshCount = electron.templates.length;
    controller.destroy();
    controller.destroy();
    mutation.resolve();
    await drain;

    expect(drained).toBe(true);
    expect(electron.templates).toHaveLength(refreshCount);
    expect(electron.destroy).toHaveBeenCalledOnce();
  });
});
