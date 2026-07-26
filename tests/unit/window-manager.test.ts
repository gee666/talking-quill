import { beforeEach, describe, expect, it, vi } from 'vitest';

const electron = vi.hoisted(() => {
  type Listener = (...args: unknown[]) => void;

  class Emitter {
    readonly listeners = new Map<string, Set<Listener>>();

    on(event: string, listener: Listener): this {
      const listeners = this.listeners.get(event) ?? new Set<Listener>();
      listeners.add(listener);
      this.listeners.set(event, listeners);
      return this;
    }

    once(event: string, listener: Listener): this {
      const onceListener: Listener = (...args) => {
        this.off(event, onceListener);
        listener(...args);
      };
      return this.on(event, onceListener);
    }

    off(event: string, listener: Listener): this {
      this.listeners.get(event)?.delete(listener);
      return this;
    }

    emit(event: string, ...args: unknown[]): void {
      for (const listener of [...(this.listeners.get(event) ?? [])]) listener(...args);
    }
  }

  let nextId = 1;
  class WebContents extends Emitter {
    readonly id = nextId++;
    send = vi.fn();
    setWindowOpenHandler = vi.fn();
    isDestroyed = () => false;
  }

  class BrowserWindow extends Emitter {
    static readonly instances: BrowserWindow[] = [];
    readonly webContents = new WebContents();
    readonly options: { readonly title?: string };
    readonly show = vi.fn();
    readonly hide = vi.fn();
    readonly focus = vi.fn();
    readonly minimize = vi.fn();
    readonly maximize = vi.fn();
    readonly unmaximize = vi.fn();
    readonly restore = vi.fn();
    readonly close = vi.fn();
    destroyed = false;

    constructor(options: { readonly title?: string }) {
      super();
      this.options = options;
      BrowserWindow.instances.push(this);
    }

    isDestroyed(): boolean {
      return this.destroyed;
    }

    destroy(): void {
      if (this.destroyed) return;
      this.destroyed = true;
      this.webContents.emit('destroyed');
    }

    isMinimized(): boolean {
      return false;
    }

    isMaximized(): boolean {
      return false;
    }

    resetForTest(): void {
      this.destroyed = false;
    }
  }

  return {
    BrowserWindow,
    reset: () => {
      BrowserWindow.instances.length = 0;
      nextId = 1;
    },
    app: {
      isPackaged: false,
      getAppPath: () => 'C:/app',
    },
    screen: {
      screenToDipPoint: (point: unknown) => point,
    },
  };
});

vi.mock('electron', () => ({
  app: electron.app,
  BrowserWindow: electron.BrowserWindow,
  screen: electron.screen,
}));

import { WindowManager } from '../../app/src/main/app/window-manager';
import type { RendererLoader } from '../../app/src/main/app/renderer-loader';
import type { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  electron.reset();
});

function createManager(requestQuit = vi.fn()): WindowManager {
  return new WindowManager(
    {
      allowsDevTools: false,
      urlFor: (role: string) => `talking-quill://app/${role}/index.html`,
      load: vi.fn(() => Promise.resolve()),
    } as unknown as RendererLoader,
    {
      register: vi.fn(),
      unregister: vi.fn(),
    } as unknown as WindowRoleRegistry,
    {
      flush: vi.fn(() => Promise.resolve()),
      get: vi.fn(() => ({ app: { closeToTray: true } })),
    } as unknown as SettingsStore,
    { requestQuit, onMaximizedChanged: vi.fn(), onMainHidden: vi.fn() },
  );
}

function mainWindows() {
  return electron.BrowserWindow.instances.filter(
    ({ options }) => options.title === 'Talking Quill',
  );
}

describe('WindowManager renderer recovery', () => {
  it('cancels pending recovery when quitting begins', async () => {
    const manager = createManager();
    await manager.createAll();
    const main = mainWindows()[0];
    expect(main).toBeDefined();
    main?.webContents.emit('did-finish-load');
    main?.webContents.emit('render-process-gone');

    manager.beginQuit();
    electron.BrowserWindow.instances[1]?.webContents.emit('did-finish-load');
    expect(vi.getTimerCount()).toBe(0);
    await vi.advanceTimersByTimeAsync(1_000);

    expect(mainWindows()).toHaveLength(1);
    await manager.createAll();
    expect(electron.BrowserWindow.instances).toHaveLength(3);
  });

  it('does not let an old stability window reset replacement retry attempts', async () => {
    const requestQuit = vi.fn();
    const manager = createManager(requestQuit);
    await manager.createAll();
    const first = mainWindows()[0];
    first?.webContents.emit('did-finish-load');
    await vi.advanceTimersByTimeAsync(29_000);
    first?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(250);

    const second = mainWindows()[1];
    second?.webContents.emit('did-finish-load');
    await vi.advanceTimersByTimeAsync(1_000);
    second?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(500);

    const third = mainWindows()[2];
    third?.webContents.emit('did-finish-load');
    third?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(1_000);

    expect(mainWindows()).toHaveLength(3);
    expect(requestQuit).toHaveBeenCalledOnce();
  });
});
