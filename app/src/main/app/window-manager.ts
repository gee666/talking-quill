import { app as electronApp, BrowserWindow, screen, type WebContents } from 'electron';
import { join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION, type WindowRole } from '../../shared/constants/app';
import { WIDGET_DIMENSIONS } from '../../shared/constants/echo-session';
import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';
import { hardenWebContents } from '../security/web-contents-policy';
import type { WindowRoleRegistry } from './window-role-registry';
import type { RendererLoader } from './renderer-loader';
import { physicalBoundsToDip } from './display-bounds';
import { widgetContentBounds } from './widget-geometry';

export interface WindowManagerCallbacks {
  readonly requestQuit: () => void;
  readonly onMaximizedChanged: (maximized: boolean) => void;
  readonly onMainHidden: () => void;
}

const MAX_RENDERER_RECOVERY_ATTEMPTS = 2;
const RENDERER_RECOVERY_BACKOFF_MS = 250;
const RENDERER_STABILITY_WINDOW_MS = 30_000;

export class WindowManager {
  readonly #loader: RendererLoader;
  readonly #roles: WindowRoleRegistry;
  readonly #settings: SettingsStore;
  readonly #callbacks: WindowManagerCallbacks;
  readonly #windows = new Map<WindowRole, BrowserWindow>();
  readonly #recoveryAttempts = new Map<WindowRole, number>();
  readonly #recoveryTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  readonly #stabilityTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  #pendingMainClose: Promise<void> | null = null;
  #quitting = false;

  constructor(
    loader: RendererLoader,
    roles: WindowRoleRegistry,
    settings: SettingsStore,
    callbacks: WindowManagerCallbacks,
  ) {
    this.#loader = loader;
    this.#roles = roles;
    this.#settings = settings;
    this.#callbacks = callbacks;
  }

  async createAll(): Promise<void> {
    await Promise.all([
      this.#createAndLoad('main'),
      this.#createAndLoad('widget'),
      this.#createAndLoad('capture'),
    ]);
  }

  getWebContents(): readonly WebContents[] {
    return [...this.#windows.values()].map((window) => window.webContents);
  }

  getByWebContentsId(id: number): BrowserWindow | null {
    return [...this.#windows.values()].find((window) => window.webContents.id === id) ?? null;
  }

  async closeMainByWebContentsId(id: number): Promise<void> {
    const main = this.#windows.get('main');
    if (main?.webContents.id !== id) return;
    await this.#coordinateMainClose(main);
  }

  showWidget(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'] = null,
  ): boolean {
    const widget = this.#windows.get('widget');
    if (widget === undefined || widget.isDestroyed()) return false;
    const displayBounds =
      targetBounds !== null && process.platform === 'win32'
        ? physicalBoundsToDip(targetBounds, (point) => screen.screenToDipPoint(point))
        : targetBounds;
    const display =
      displayBounds === null
        ? screen.getDisplayNearestPoint(screen.getCursorScreenPoint())
        : screen.getDisplayMatching(displayBounds);
    widget.setContentBounds(widgetContentBounds(size, display.workArea), false);
    // Preserve renderer-selected hit testing across screenshot-only hide/show
    // cycles so a stationary pointer can still click Stop or Cancel.
    widget.showInactive();
    return true;
  }

  isWidgetVisible(): boolean {
    const widget = this.#windows.get('widget');
    return widget !== undefined && !widget.isDestroyed() && widget.isVisible();
  }

  hideWidget(preserveInteraction = false): void {
    const widget = this.#windows.get('widget');
    if (widget !== undefined && !widget.isDestroyed()) {
      widget.setFocusable(false);
      if (!preserveInteraction) {
        widget.setIgnoreMouseEvents(true, { forward: true });
      }
      widget.hide();
    }
  }

  setWidgetInteractive(webContentsId: number, interactive: boolean): void {
    const widget = this.#windows.get('widget');
    if (widget?.webContents.id !== webContentsId || widget.isDestroyed()) return;
    widget.setFocusable(false);
    widget.setIgnoreMouseEvents(!interactive, { forward: !interactive });
  }

  showMain(): void {
    const main = this.#windows.get('main');
    if (main === undefined || main.isDestroyed()) return;
    if (main.isMinimized()) main.restore();
    main.show();
    main.focus();
  }

  beginQuit(): void {
    if (this.#quitting) return;
    this.#quitting = true;
    this.#clearTimers(this.#recoveryTimers);
    this.#clearTimers(this.#stabilityTimers);
  }

  destroyAll(): void {
    this.beginQuit();
    for (const window of this.#windows.values()) {
      if (!window.isDestroyed()) window.destroy();
    }
    this.#windows.clear();
  }

  async #createAndLoad(role: WindowRole): Promise<void> {
    if (this.#quitting) return;
    const window = this.#create(role);
    this.#windows.set(role, window);
    const expectedUrl = this.#loader.urlFor(role);
    this.#roles.register(window.webContents, role, expectedUrl);
    hardenWebContents(window.webContents, expectedUrl);
    this.#attachRecovery(window, role);
    await this.#loader.load(window, role);
  }

  #create(role: WindowRole): BrowserWindow {
    const common: Electron.BrowserWindowConstructorOptions = {
      show: false,
      frame: false,
      backgroundColor: '#161B23',
      icon: electronApp.isPackaged
        ? join(process.resourcesPath, 'app-icon.png')
        : join(electronApp.getAppPath(), 'assets', 'app-icon.png'),
      webPreferences: {
        preload: join(__dirname, '..', 'preload', `${role}.js`),
        sandbox: true,
        contextIsolation: true,
        nodeIntegration: false,
        nodeIntegrationInWorker: false,
        nodeIntegrationInSubFrames: false,
        webSecurity: true,
        allowRunningInsecureContent: false,
        webviewTag: false,
        devTools: this.#loader.allowsDevTools,
        partition: role === 'capture' ? CAPTURE_PARTITION : UI_PARTITION,
        backgroundThrottling: role !== 'capture',
      },
    };

    if (role === 'main') {
      const window = new BrowserWindow({
        ...common,
        title: 'Talking Quill',
        width: 1100,
        height: 720,
        minWidth: 960,
        minHeight: 600,
      });
      window.once('ready-to-show', () => {
        if (!this.#quitting && !window.isDestroyed()) window.show();
      });
      window.on('close', (event) => {
        if (this.#quitting) return;
        event.preventDefault();
        void this.#coordinateMainClose(window);
      });
      window.on('maximize', () => this.#callbacks.onMaximizedChanged(true));
      window.on('unmaximize', () => this.#callbacks.onMaximizedChanged(false));
      return window;
    }

    if (role === 'widget') {
      return new BrowserWindow({
        ...common,
        title: 'Talking Quill Widget',
        // An opaque background colour defeats `transparent`, so the widget window
        // must clear it for the floating pill to sit directly on the desktop.
        backgroundColor: '#00000000',
        width: WIDGET_DIMENSIONS.default.width,
        height: WIDGET_DIMENSIONS.default.height,
        resizable: false,
        hasShadow: false,
        transparent: true,
        alwaysOnTop: true,
        focusable: false,
        skipTaskbar: true,
      });
    }

    return new BrowserWindow({
      ...common,
      title: 'Talking Quill Capture',
      width: 1,
      height: 1,
      resizable: false,
      focusable: false,
      skipTaskbar: true,
    });
  }

  #coordinateMainClose(window: BrowserWindow): Promise<void> {
    if (this.#quitting || window.isDestroyed()) return Promise.resolve();
    if (this.#pendingMainClose !== null) return this.#pendingMainClose;

    const decision = this.#settings.flush().then(
      () => {
        if (this.#quitting || window.isDestroyed()) return;
        if (this.#settings.get().app.closeToTray) {
          window.hide();
          this.#callbacks.onMainHidden();
        } else this.#callbacks.requestQuit();
      },
      () => {
        if (this.#quitting || window.isDestroyed()) return;
        window.show();
        window.focus();
      },
    );
    this.#pendingMainClose = decision.finally(() => {
      this.#pendingMainClose = null;
    });
    return this.#pendingMainClose;
  }

  #attachRecovery(window: BrowserWindow, role: WindowRole): void {
    let loaded = false;
    window.webContents.once('did-finish-load', () => {
      if (this.#quitting || this.#windows.get(role) !== window) return;
      loaded = true;
      this.#clearRoleTimer(this.#stabilityTimers, role);
      const timer = setTimeout(() => {
        if (this.#stabilityTimers.get(role) !== timer) return;
        this.#stabilityTimers.delete(role);
        if (!this.#quitting && this.#windows.get(role) === window) {
          this.#recoveryAttempts.delete(role);
        }
      }, RENDERER_STABILITY_WINDOW_MS);
      this.#stabilityTimers.set(role, timer);
      timer.unref();
    });
    window.webContents.on('did-fail-load', (_event, errorCode) => {
      if (errorCode !== -3) this.#recover(role, window);
    });
    window.webContents.on('render-process-gone', () => {
      if (loaded) this.#recover(role, window);
    });
  }

  #recover(role: WindowRole, failed: BrowserWindow): void {
    if (this.#quitting || this.#windows.get(role) !== failed) return;
    this.#clearRoleTimer(this.#stabilityTimers, role);
    const attempts = (this.#recoveryAttempts.get(role) ?? 0) + 1;
    this.#recoveryAttempts.set(role, attempts);
    this.#windows.delete(role);
    this.#roles.unregister(failed.webContents.id);
    if (!failed.isDestroyed()) failed.destroy();
    if (attempts > MAX_RENDERER_RECOVERY_ATTEMPTS) {
      this.#callbacks.requestQuit();
      return;
    }
    this.#clearRoleTimer(this.#recoveryTimers, role);
    const timer = setTimeout(() => {
      if (this.#recoveryTimers.get(role) !== timer) return;
      this.#recoveryTimers.delete(role);
      if (this.#quitting) return;
      void this.#createAndLoad(role).catch(() => undefined);
    }, RENDERER_RECOVERY_BACKOFF_MS * attempts);
    this.#recoveryTimers.set(role, timer);
    timer.unref();
  }

  #clearRoleTimer(timers: Map<WindowRole, ReturnType<typeof setTimeout>>, role: WindowRole): void {
    const timer = timers.get(role);
    if (timer === undefined) return;
    clearTimeout(timer);
    timers.delete(role);
  }

  #clearTimers(timers: Map<WindowRole, ReturnType<typeof setTimeout>>): void {
    for (const timer of timers.values()) clearTimeout(timer);
    timers.clear();
  }
}
