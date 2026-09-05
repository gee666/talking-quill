import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ ipcMain: {} }));

import { createHandlers, type HandlerDependencies } from '../../app/src/main/ipc/handlers';
import { registerIpcTransport, type IpcMainRegistrar } from '../../app/src/main/ipc/transport';
import type { InvokeHandlerMap } from '../../app/src/main/ipc/types';
import { invokeRegistry } from '../../app/src/shared/ipc/registry';
import { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';

const context = { webContentsId: 41, onDestroyed: () => () => undefined };

describe('IPC handler composition', () => {
  it('covers every invoke channel and registers in registry order, not handler-map order', () => {
    const handlers = createHandlers({} as HandlerDependencies);
    expect(Object.keys(handlers).sort()).toEqual(Object.keys(invokeRegistry).sort());
    const reversed = Object.fromEntries(Object.entries(handlers).reverse()) as InvokeHandlerMap;
    const registered: string[] = [];
    const registrar: IpcMainRegistrar = {
      handle(channel) {
        registered.push(channel);
      },
      removeHandler: vi.fn(),
    };

    const transport = registerIpcTransport(new WindowRoleRegistry(), reversed, registrar);

    expect(registered).toEqual(Object.keys(invokeRegistry));
    transport.dispose();
  });

  it.each([
    ['info:export-diagnostics', 'Diagnostic export dialog owner is unavailable'],
    ['profile:import-file', 'Dictation profile dialog owner is unavailable'],
    ['profile:export-file', 'Dictation profile dialog owner is unavailable'],
    ['provider:pi-installation-browse', 'Pi installation dialog owner is unavailable'],
    ['commands:import-file', 'Voice command dialog owner is unavailable'],
    ['commands:export-file', 'Voice command dialog owner is unavailable'],
    ['vocabulary:import-file', 'Vocabulary dialog owner is unavailable'],
    ['vocabulary:export-file', 'Vocabulary dialog owner is unavailable'],
  ] as const)('rejects %s when its renderer has no owning window', async (channel, message) => {
    const getByWebContentsId = vi.fn(() => null);
    const handlers = createHandlers({
      windows: { getByWebContentsId },
    } as unknown as HandlerDependencies);

    await expect(Promise.resolve().then(() => handlers[channel]({}, context))).rejects.toThrow(
      message,
    );

    expect(getByWebContentsId).toHaveBeenCalledExactlyOnceWith(context.webContentsId);
  });
});
