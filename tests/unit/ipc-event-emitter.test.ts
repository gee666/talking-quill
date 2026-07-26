import type { WebContents } from 'electron';
import { describe, expect, it, vi } from 'vitest';
import { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';
import type { WindowRole } from '../../app/src/shared/constants/app';

function target(id: number, options: { destroyed?: boolean; throws?: boolean } = {}) {
  const send = options.throws
    ? vi.fn(() => {
        throw new Error('renderer disappeared');
      })
    : vi.fn();
  return {
    webContents: {
      id,
      isDestroyed: () => options.destroyed ?? false,
      send,
    } as unknown as WebContents,
    send,
  };
}

describe('IpcEventEmitter', () => {
  it('validates payloads, filters roles, and isolates target delivery failures', () => {
    const allowed = target(1);
    const denied = target(2);
    const unregistered = target(3);
    const destroyed = target(4, { destroyed: true });
    const failing = target(5, { throws: true });
    const later = target(6);
    const roles = new Map<number, WindowRole>([
      [1, 'main'],
      [2, 'capture'],
      [4, 'main'],
      [5, 'main'],
      [6, 'main'],
    ]);
    const registry = {
      get: (id: number) => {
        const role = roles.get(id);
        return role === undefined ? null : { role, expectedUrl: 'talking-quill://app' };
      },
    } as unknown as WindowRoleRegistry;
    const emitter = new IpcEventEmitter(registry, () => [
      allowed.webContents,
      denied.webContents,
      unregistered.webContents,
      destroyed.webContents,
      failing.webContents,
      later.webContents,
    ]);

    emitter.send('window:maximized-changed', { maximized: true });

    expect(allowed.send).toHaveBeenCalledWith('window:maximized-changed', { maximized: true });
    expect(denied.send).not.toHaveBeenCalled();
    expect(unregistered.send).not.toHaveBeenCalled();
    expect(destroyed.send).not.toHaveBeenCalled();
    expect(failing.send).toHaveBeenCalledOnce();
    expect(later.send).toHaveBeenCalledOnce();
    expect(() => emitter.send('window:maximized-changed', { maximized: 'yes' } as never)).toThrow();
  });
});
