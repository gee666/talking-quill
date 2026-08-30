import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';

vi.mock('electron', () => ({ ipcMain: {} }));
vi.mock('../../app/src/main/security/ipc-authorization', () => ({ authorizeIpc: vi.fn() }));

import { registerIpcTransport } from '../../app/src/main/ipc/transport';
import type { InvokeHandlerMap } from '../../app/src/main/ipc/types';

class FakeRegistrar {
  readonly listeners = new Map<string, (event: never, input: unknown) => Promise<unknown>>();

  handle(channel: string, listener: (event: never, input: unknown) => Promise<unknown>): void {
    this.listeners.set(channel, listener);
  }

  removeHandler(channel: string): void {
    this.listeners.delete(channel);
  }
}

class FakeSender extends EventEmitter {
  readonly id = 7;
  readonly mainFrame = { url: 'talking-quill://app/main/index.html' };

  isDestroyed(): boolean {
    return false;
  }
}

describe('IPC transport shutdown', () => {
  it('invalidates a stuck invocation before draining it', async () => {
    const registrar = new FakeRegistrar();
    const sender = new FakeSender();
    const cancelled = vi.fn();
    const handlers = {
      'bootstrap:get': (
        _request: unknown,
        context: { onDestroyed(listener: () => void): () => void },
      ) =>
        new Promise<never>((_resolve, reject) => {
          context.onDestroyed(() => {
            cancelled();
            reject(new Error('cancelled'));
          });
        }),
    } as unknown as InvokeHandlerMap;
    const transport = registerIpcTransport(
      { get: vi.fn(() => null) } as never,
      handlers,
      registrar,
    );
    const listener = registrar.listeners.get('bootstrap:get');
    if (listener === undefined) throw new Error('bootstrap handler was not registered');

    const invocation = listener(
      {
        sender,
        senderFrame: sender.mainFrame,
      } as never,
      {},
    );
    await Promise.resolve();
    expect(transport.pendingChannels()).toEqual(['bootstrap:get']);

    transport.stopAccepting();
    await expect(invocation).resolves.toMatchObject({ ok: false });
    await expect(transport.drain()).resolves.toBeUndefined();
    expect(cancelled).toHaveBeenCalledOnce();
    expect(transport.pendingChannels()).toEqual([]);
  });
});
