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
    invalidate = vi.fn();
    isDestroyed = () => false;
  }

  class BrowserWindow extends Emitter {
    static readonly instances: BrowserWindow[] = [];
    readonly webContents = new WebContents();
    readonly options: { readonly title?: string };
    readonly show = vi.fn();
    readonly hide = vi.fn(() => {
      this.visible = false;
    });
    readonly focus = vi.fn();
    readonly minimize = vi.fn();
    readonly maximize = vi.fn();
    readonly unmaximize = vi.fn();
    readonly restore = vi.fn();
    readonly close = vi.fn();
    readonly setContentBounds = vi.fn();
    readonly setFocusable = vi.fn();
    readonly setIgnoreMouseEvents = vi.fn();
    readonly setAlwaysOnTop = vi.fn();
    readonly moveTop = vi.fn();
    readonly showInactive = vi.fn(() => {
      this.visible = true;
    });
    destroyed = false;
    visible = false;

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

    isVisible(): boolean {
      return this.visible;
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
      getCursorScreenPoint: () => ({ x: 0, y: 0 }),
      getDisplayNearestPoint: () => ({
        workArea: { x: 0, y: 0, width: 1_920, height: 1_080 },
      }),
      getDisplayMatching: () => ({
        workArea: { x: 0, y: 0, width: 1_920, height: 1_080 },
      }),
    },
  };
});

vi.mock('electron', () => ({
  app: electron.app,
  BrowserWindow: electron.BrowserWindow,
  screen: electron.screen,
}));

import { WidgetCaptureExclusion } from '../../app/src/main/app/widget-capture-exclusion';
import { RENDERER_LOAD_TIMEOUT_MS, WindowManager } from '../../app/src/main/app/window-manager';
import type { RendererLoader } from '../../app/src/main/app/renderer-loader';
import type { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';

beforeEach(() => {
  vi.useFakeTimers();
  vi.clearAllMocks();
  electron.reset();
});

function createManager(
  requestQuit = vi.fn(),
  load: RendererLoader['load'] = vi.fn(() => Promise.resolve()),
): WindowManager {
  return new WindowManager(
    {
      allowsDevTools: false,
      urlFor: (role: string) => `talking-quill://app/${role}/index.html`,
      load,
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

function widgetWindows() {
  return electron.BrowserWindow.instances.filter(
    ({ options }) => options.title === 'Talking Quill Widget',
  );
}

describe('WindowManager renderer recovery', () => {
  it('reasserts topmost presentation and invalidates the transparent widget surface', async () => {
    const manager = createManager();
    await manager.createAll();
    expect(widgetWindows()).toHaveLength(0);
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const widget = widgetWindows()[0];

    expect(manager.showWidget('default', null)).toBe(true);

    expect(widget?.setAlwaysOnTop).toHaveBeenCalledWith(true);
    expect(widget?.showInactive).toHaveBeenCalledOnce();
    expect(widget?.moveTop).toHaveBeenCalledOnce();
    expect(widget?.webContents.invalidate).toHaveBeenCalledOnce();
  });

  it('recreates the widget for every activation', async () => {
    const manager = createManager();
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const first = widgetWindows()[0];
    manager.showWidget('default', null);
    manager.hideWidget();

    expect(await manager.prepareWidgetForActivation()).toBe(true);

    expect(first?.destroyed).toBe(true);
    expect(widgetWindows()).toHaveLength(2);
  });

  it('marks a failed native show for replacement on the next activation', async () => {
    const manager = createManager();
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const first = widgetWindows()[0];
    first?.showInactive.mockImplementationOnce(() => undefined);

    expect(manager.showWidget('default', null)).toBe(false);
    expect(await manager.prepareWidgetForActivation()).toBe(true);

    expect(first?.destroyed).toBe(true);
    expect(widgetWindows()).toHaveLength(2);
  });

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
    expect(electron.BrowserWindow.instances).toHaveLength(2);
  });

  it('recovers a renderer crash before its first did-finish-load event', async () => {
    const manager = createManager();
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const first = widgetWindows()[0];

    first?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(250);

    expect(first?.destroyed).toBe(true);
    expect(widgetWindows()).toHaveLength(2);
  });

  it('restores desired widget visibility after replacing its renderer', async () => {
    const manager = createManager();
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const first = widgetWindows()[0];
    first?.webContents.emit('did-finish-load');
    expect(manager.showWidget('large', { x: 10, y: 20, width: 800, height: 600 })).toBe(true);
    expect(first?.showInactive).toHaveBeenCalledOnce();

    first?.webContents.emit('render-process-gone');
    expect(manager.isWidgetVisible()).toBe(true);
    await vi.advanceTimersByTimeAsync(250);

    const replacement = widgetWindows()[1];
    expect(replacement).toBeDefined();
    expect(replacement?.showInactive).toHaveBeenCalledOnce();
    expect(replacement?.setContentBounds).toHaveBeenCalledOnce();
  });

  it('does not let late capture restoration override a terminal widget hide', async () => {
    let resolveFrontApp!: (value: {
      processName: string;
      windowTitle: string;
      windowBounds: { x: number; y: number; width: number; height: number };
    }) => void;
    const frontApp = new Promise<{
      processName: string;
      windowTitle: string;
      windowBounds: { x: number; y: number; width: number; height: number };
    }>((resolve) => {
      resolveFrontApp = resolve;
    });
    const manager = createManager();
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const widget = widgetWindows()[0];
    manager.showWidget('default', null);
    const exclusion = new WidgetCaptureExclusion({
      windows: manager,
      getWidgetSize: () => 'large',
      getFrontApp: () => frontApp,
    });

    await exclusion.setExcluded(true);
    const restoration = exclusion.setExcluded(false);
    await Promise.resolve();
    manager.hideWidget();
    resolveFrontApp({
      processName: 'target',
      windowTitle: 'document',
      windowBounds: { x: 10, y: 20, width: 800, height: 600 },
    });
    await restoration;

    expect(manager.isWidgetVisible()).toBe(false);
    expect(widget?.showInactive).toHaveBeenCalledOnce();
  });

  it('keeps a not-yet-loaded replacement hidden for capture, then restores it', async () => {
    let finishReplacementLoad!: () => void;
    const load = vi.fn<RendererLoader['load']>((_window, role) => {
      if (role !== 'widget' || widgetWindows().length < 2) return Promise.resolve();
      return new Promise<void>((resolve) => {
        finishReplacementLoad = resolve;
      });
    });
    const manager = createManager(vi.fn(), load);
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);
    const first = widgetWindows()[0];
    first?.webContents.emit('did-finish-load');
    manager.showWidget('default', null);
    first?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(250);

    const replacement = widgetWindows()[1];
    expect(replacement).toBeDefined();
    const exclusion = new WidgetCaptureExclusion({
      windows: manager,
      getWidgetSize: () => 'large',
      getFrontApp: () =>
        Promise.resolve({
          processName: 'target',
          windowTitle: 'document',
          windowBounds: { x: 10, y: 20, width: 800, height: 600 },
        }),
    });
    await exclusion.setExcluded(true);
    finishReplacementLoad();
    await Promise.resolve();
    await Promise.resolve();
    expect(replacement?.showInactive).not.toHaveBeenCalled();

    await exclusion.setExcluded(false);
    expect(replacement?.showInactive).toHaveBeenCalledOnce();
  });

  it('bounds rejected replacement loads and requests quit after retry exhaustion', async () => {
    const requestQuit = vi.fn();
    let widgetLoads = 0;
    const load = vi.fn<RendererLoader['load']>((_window, role) => {
      if (role !== 'widget') return Promise.resolve();
      widgetLoads += 1;
      return widgetLoads === 1
        ? Promise.resolve()
        : Promise.reject(new Error('renderer load rejected'));
    });
    const manager = createManager(requestQuit, load);
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);

    widgetWindows()[0]?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(250);
    await vi.advanceTimersByTimeAsync(500);

    expect(widgetWindows()).toHaveLength(3);
    expect(requestQuit).toHaveBeenCalledOnce();
  });

  it('times out never-settling replacement loads and stops after bounded retries', async () => {
    const requestQuit = vi.fn();
    let widgetLoads = 0;
    const load = vi.fn<RendererLoader['load']>((_window, role) => {
      if (role !== 'widget') return Promise.resolve();
      widgetLoads += 1;
      return widgetLoads === 1 ? Promise.resolve() : new Promise<void>(() => undefined);
    });
    const manager = createManager(requestQuit, load);
    await manager.createAll();
    expect(await manager.prepareWidgetForActivation()).toBe(true);

    widgetWindows()[0]?.webContents.emit('render-process-gone');
    await vi.advanceTimersByTimeAsync(250);
    expect(widgetWindows()).toHaveLength(2);

    await vi.advanceTimersByTimeAsync(RENDERER_LOAD_TIMEOUT_MS);
    await vi.advanceTimersByTimeAsync(500);
    expect(widgetWindows()).toHaveLength(3);

    await vi.advanceTimersByTimeAsync(RENDERER_LOAD_TIMEOUT_MS);
    expect(requestQuit).toHaveBeenCalledOnce();
    expect(widgetWindows()).toHaveLength(3);
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
