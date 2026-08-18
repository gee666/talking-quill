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
import type { WidgetVisibilityLease } from './widget-capture-exclusion';
import { widgetContentBounds } from './widget-geometry';

export interface WindowManagerCallbacks {
  readonly requestQuit: () => void;
  readonly onMaximizedChanged: (maximized: boolean) => void;
  readonly onMainHidden: () => void;
}

const MAX_RENDERER_RECOVERY_ATTEMPTS = 2;
const RENDERER_RECOVERY_BACKOFF_MS = 250;
const RENDERER_STABILITY_WINDOW_MS = 30_000;
export const RENDERER_LOAD_TIMEOUT_MS = 10_000;

interface DesiredWidgetVisibility {
  readonly size: Settings['app']['widgetSize'];
  readonly targetBounds: HelperFrontApp['windowBounds'];
}

export class WindowManager {
  readonly #loader: RendererLoader;
  readonly #roles: WindowRoleRegistry;
  readonly #settings: SettingsStore;
  readonly #callbacks: WindowManagerCallbacks;
  readonly #windows = new Map<WindowRole, BrowserWindow>();
  readonly #recoveryAttempts = new Map<WindowRole, number>();
  readonly #recoveryTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  readonly #stabilityTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  readonly #pendingRendererLoads = new Map<BrowserWindow, () => void>();
  #desiredWidgetVisibility: DesiredWidgetVisibility | null = null;
  #widgetVisibilityGeneration = 0;
  #widgetExcludedFromCapture = false;
  #widgetCreation: Promise<boolean> | null = null;
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
    // The widget is intentionally not preloaded. It is created only for an active session.
    await Promise.all([this.#createAndLoad('main'), this.#createAndLoad('capture')]);
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

  /** Creates the renderer and native transparent window for this activation. */
  createWidgetForActivation(): Promise<boolean> {
    const widget = this.#windows.get('widget');
    if (widget !== undefined && !widget.isDestroyed()) return Promise.resolve(true);
    if (this.#widgetCreation !== null) return this.#widgetCreation;
    const creation = this.#createWidget().finally(() => {
      if (this.#widgetCreation === creation) this.#widgetCreation = null;
    });
    this.#widgetCreation = creation;
    return creation;
  }

  showWidget(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'] = null,
  ): boolean {
    this.#widgetVisibilityGeneration += 1;
    this.#setDesiredWidgetVisibility(size, targetBounds);
    return this.#showDesiredWidget();
  }

  isWidgetVisible(): boolean {
    const widget = this.#windows.get('widget');
    const visible = widget !== undefined && !widget.isDestroyed() && widget.isVisible();
    // A renderer recovery gap, including a not-yet-loaded replacement, must not make screenshot
    // exclusion forget that an active session expects the widget to become visible.
    return visible || (this.#desiredWidgetVisibility !== null && !this.#widgetExcludedFromCapture);
  }

  acquireWidgetVisibilityLease(): WidgetVisibilityLease | null {
    return this.isWidgetVisible()
      ? Object.freeze({ generation: this.#widgetVisibilityGeneration })
      : null;
  }

  restoreWidgetVisibility(
    lease: WidgetVisibilityLease | null,
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): boolean {
    if (
      (lease !== null && lease.generation !== this.#widgetVisibilityGeneration) ||
      this.#desiredWidgetVisibility === null
    ) {
      return false;
    }
    if (lease !== null) {
      this.#desiredWidgetVisibility = {
        size,
        targetBounds: targetBounds === null ? null : { ...targetBounds },
      };
    }
    this.#widgetExcludedFromCapture = false;
    return this.#showDesiredWidget();
  }

  excludeWidgetFromCapture(): void {
    this.#widgetExcludedFromCapture = true;
    const widget = this.#windows.get('widget');
    if (widget !== undefined && !widget.isDestroyed()) {
      widget.setFocusable(false);
      widget.hide();
    }
  }

  removeWidget(): void {
    this.#widgetVisibilityGeneration += 1;
    this.#desiredWidgetVisibility = null;
    this.#widgetExcludedFromCapture = false;
    this.#destroyWidgetWindow();
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
    for (const invalidate of [...this.#pendingRendererLoads.values()]) invalidate();
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

  async #createAndLoad(role: WindowRole): Promise<boolean> {
    if (this.#quitting) return false;
    const window = this.#create(role);
    this.#windows.set(role, window);
    const expectedUrl = this.#loader.urlFor(role);
    this.#roles.register(window.webContents, role, expectedUrl);
    hardenWebContents(window.webContents, expectedUrl);
    this.#attachRecovery(window, role);
    const loadOutcome = await this.#loadRenderer(window, role);
    if (loadOutcome === 'failed') {
      this.#recover(role, window);
      return false;
    }
    if (loadOutcome === 'loaded') {
      this.#restoreDesiredWidgetAfterLoad(role, window);
      return true;
    }
    return false;
  }

  async #createWidget(): Promise<boolean> {
    if (this.#quitting) return false;
    this.#destroyWidgetWindow();
    return this.#createAndLoad('widget');
  }

  #destroyWidgetWindow(): void {
    const widget = this.#windows.get('widget');
    if (widget !== undefined) {
      this.#pendingRendererLoads.get(widget)?.();
      this.#windows.delete('widget');
      this.#roles.unregister(widget.webContents.id);
      if (!widget.isDestroyed()) widget.destroy();
    }
    this.#clearRoleTimer(this.#recoveryTimers, 'widget');
    this.#clearRoleTimer(this.#stabilityTimers, 'widget');
    this.#recoveryAttempts.delete('widget');
  }

  #loadRenderer(
    window: BrowserWindow,
    role: WindowRole,
  ): Promise<'loaded' | 'failed' | 'invalidated'> {
    return new Promise((resolve) => {
      let finished = false;
      let timer: ReturnType<typeof setTimeout> | null = null;
      const finish = (outcome: 'loaded' | 'failed' | 'invalidated'): void => {
        if (finished) return;
        finished = true;
        if (timer !== null) clearTimeout(timer);
        if (this.#pendingRendererLoads.get(window) === invalidate) {
          this.#pendingRendererLoads.delete(window);
        }
        resolve(outcome);
      };
      const invalidate = (): void => finish('invalidated');
      this.#pendingRendererLoads.set(window, invalidate);
      timer = setTimeout(() => finish('failed'), RENDERER_LOAD_TIMEOUT_MS);
      timer.unref();
      try {
        void this.#loader.load(window, role).then(
          () => finish('loaded'),
          () => finish('failed'),
        );
      } catch {
        finish('failed');
      }
    });
  }

  #setDesiredWidgetVisibility(
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): void {
    this.#desiredWidgetVisibility = {
      size,
      targetBounds: targetBounds === null ? null : { ...targetBounds },
    };
  }

  #restoreDesiredWidgetAfterLoad(role: WindowRole, window: BrowserWindow): void {
    if (
      role === 'widget' &&
      !this.#quitting &&
      this.#windows.get(role) === window &&
      !this.#widgetExcludedFromCapture
    ) {
      this.#showDesiredWidget();
    }
  }

  #showDesiredWidget(): boolean {
    const desired = this.#desiredWidgetVisibility;
    const widget = this.#windows.get('widget');
    if (desired === null || widget === undefined || widget.isDestroyed()) return false;
    if (this.#widgetExcludedFromCapture) return true;
    const displayBounds =
      desired.targetBounds !== null && process.platform === 'win32'
        ? physicalBoundsToDip(desired.targetBounds, (point) => screen.screenToDipPoint(point))
        : desired.targetBounds;
    const display =
      displayBounds === null
        ? screen.getDisplayNearestPoint(screen.getCursorScreenPoint())
        : screen.getDisplayMatching(displayBounds);
    widget.setContentBounds(widgetContentBounds(desired.size, display.workArea), false);
    // Windows can drop the topmost band or retain a stale transparent surface after display
    // sleep/lock. Reassert native presentation and explicitly request a compositor frame.
    widget.setAlwaysOnTop(true);
    widget.showInactive();
    widget.moveTop();
    widget.webContents.invalidate();
    // Preserve renderer-selected hit testing across screenshot-only hide/show cycles so a
    // stationary pointer can still click Stop or Cancel.
    return widget.isVisible();
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
        // Keep the active widget responsive while the user's foreground app has focus.
        backgroundThrottling: role === 'main',
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
    window.webContents.once('did-finish-load', () => {
      if (this.#quitting || this.#windows.get(role) !== window) return;
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
    window.webContents.on('render-process-gone', () => this.#recover(role, window));
    window.on('unresponsive', () => {
      if (role === 'widget') this.#recover(role, window);
    });
  }

  #recover(role: WindowRole, failed: BrowserWindow): void {
    if (this.#quitting || this.#windows.get(role) !== failed) return;
    this.#pendingRendererLoads.get(failed)?.();
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
