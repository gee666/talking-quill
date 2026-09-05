import { createNativeWindow } from './native-window';
import { RendererRecovery } from './renderer-recovery';
import type { BrowserWindow, WebContents } from 'electron';
import type { WindowRole } from '../../shared/constants/app';
import type { HelperFrontApp } from '../../shared/helper/protocol';
import type { Settings } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';
import { hardenWebContents } from '../security/web-contents-policy';
import type { WindowRoleRegistry } from './window-role-registry';
import type { RendererLoader } from './renderer-loader';
import type { WidgetVisibilityLease } from './widget-capture-exclusion';
import { WidgetPresentation } from './widget-presentation';

export interface WindowManagerCallbacks {
  readonly requestQuit: () => void;
  readonly onMaximizedChanged: (maximized: boolean) => void;
  readonly onMainHidden: () => void;
  readonly showMainOnFirstLoad: boolean;
}

export { RENDERER_LOAD_TIMEOUT_MS } from './renderer-recovery';

export class WindowManager {
  readonly #loader: RendererLoader;
  readonly #roles: WindowRoleRegistry;
  readonly #settings: SettingsStore;
  readonly #callbacks: WindowManagerCallbacks;
  readonly #windows = new Map<WindowRole, BrowserWindow>();
  readonly #recovery: RendererRecovery;
  readonly #widget: WidgetPresentation;
  #widgetCreation: Promise<boolean> | null = null;
  #pendingMainClose: Promise<void> | null = null;
  #mainInitialLoadHandled = false;
  #foregroundAllowed: boolean;
  #explicitMainShowPending = false;
  #quitting = false;

  constructor(
    loader: RendererLoader,
    roles: WindowRoleRegistry,
    settings: SettingsStore,
    callbacks: WindowManagerCallbacks,
  ) {
    this.#widget = new WidgetPresentation(() => this.#windows.get('widget'));
    this.#loader = loader;
    this.#roles = roles;
    this.#settings = settings;
    this.#callbacks = callbacks;
    this.#foregroundAllowed = callbacks.showMainOnFirstLoad;
    this.#recovery = new RendererRecovery({
      windows: this.#windows,
      roles,
      loader,
      requestQuit: () => this.#callbacks.requestQuit(),
      onMainFailed: () => {
        this.#mainInitialLoadHandled = true;
      },
      createAndLoad: (role) => this.#createAndLoad(role),
    });
  }

  async createAll(): Promise<void> {
    // Load the non-focusable widget before native activation can be enabled. Keeping it hidden
    // avoids creating a foreground-capable surface in the middle of an activation gesture.
    const [mainReady, captureReady, widgetReady] = await Promise.all([
      this.#createAndLoad('main'),
      this.#createAndLoad('capture'),
      this.#createAndLoad('widget'),
    ]);
    if (this.#quitting) return;
    if (!mainReady || !captureReady || !widgetReady) {
      throw new Error('An application renderer could not be prepared');
    }
  }

  isMainVisible(): boolean {
    const main = this.#windows.get('main');
    return main !== undefined && !main.isDestroyed() && main.isVisible();
  }

  hasPersistentWindowRoles(): boolean {
    return (['main', 'capture', 'widget'] as const).every((role) => {
      const window = this.#windows.get(role);
      return window !== undefined && !window.isDestroyed();
    });
  }

  markRendererReady(role: 'capture' | 'widget', webContentsId: number): void {
    const window = this.#windows.get(role);
    if (window === undefined || window.isDestroyed() || window.webContents.id !== webContentsId) {
      return;
    }
    this.#recovery.markReady(webContentsId);
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
    return this.#widget.showWidget(size, targetBounds);
  }

  isWidgetVisible(): boolean {
    return this.#widget.isWidgetVisible();
  }

  acquireWidgetVisibilityLease(): WidgetVisibilityLease | null {
    return this.#widget.acquireWidgetVisibilityLease();
  }

  restoreWidgetVisibility(
    lease: WidgetVisibilityLease | null,
    size: Settings['app']['widgetSize'],
    targetBounds: HelperFrontApp['windowBounds'],
  ): boolean {
    return this.#widget.restoreWidgetVisibility(lease, size, targetBounds);
  }

  excludeWidgetFromCapture(): void {
    this.#widget.excludeWidgetFromCapture();
  }

  removeWidget(): void {
    this.#widget.removeWidget();
  }

  setWidgetInteractive(webContentsId: number, interactive: boolean): void {
    this.#widget.setWidgetInteractive(webContentsId, interactive);
  }

  showMain(): void {
    if (!this.#foregroundAllowed) return;
    this.#showMain();
  }

  showMainByUser(): void {
    this.#foregroundAllowed = true;
    this.#explicitMainShowPending = true;
    this.#showMain();
  }

  #showMain(): void {
    const main = this.#windows.get('main');
    if (main === undefined || main.isDestroyed()) return;
    if (main.isMinimized()) main.restore();
    main.show();
    main.focus();
    this.#explicitMainShowPending = false;
  }

  beginQuit(): void {
    if (this.#quitting) return;
    this.#quitting = true;
    this.#recovery.stop();
  }

  destroyAll(): void {
    this.beginQuit();
    for (const window of this.#windows.values()) {
      if (!window.isDestroyed()) window.destroy();
    }
    this.#windows.clear();
    this.#recovery.clearReady();
  }

  async #createAndLoad(role: WindowRole): Promise<boolean> {
    if (this.#quitting) return false;
    const window = this.#create(role);
    this.#windows.set(role, window);
    const expectedUrl = this.#loader.urlFor(role);
    this.#roles.register(window.webContents, role, expectedUrl);
    hardenWebContents(window.webContents, expectedUrl);
    this.#recovery.attachRecovery(window, role);
    const loadOutcome = await this.#recovery.loadRenderer(window, role);
    if (loadOutcome === 'failed') {
      this.#recovery.recover(role, window);
      return false;
    }
    if (loadOutcome === 'loaded') {
      if (role !== 'main' && !(await this.#recovery.waitForRendererReady(window))) return false;
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
      this.#recovery.invalidate(widget);
      this.#recovery.forgetRendererReady(widget);
      this.#windows.delete('widget');
      this.#roles.unregister(widget.webContents.id);
      if (!widget.isDestroyed()) widget.destroy();
    }
    this.#recovery.resetRole('widget');
  }

  #restoreDesiredWidgetAfterLoad(role: WindowRole, window: BrowserWindow): void {
    if (
      role === 'widget' &&
      !this.#quitting &&
      this.#windows.get(role) === window &&
      !this.#widget.excludedFromCapture
    ) {
      this.#widget.showDesiredWidget();
    }
  }

  #create(role: WindowRole): BrowserWindow {
    const window = createNativeWindow(role, this.#loader.allowsDevTools);
    if (role === 'main') {
      window.once('ready-to-show', () => {
        const initialLoad = !this.#mainInitialLoadHandled;
        this.#mainInitialLoadHandled = true;
        if (this.#explicitMainShowPending) {
          this.#showMain();
          return;
        }
        if (
          initialLoad &&
          this.#callbacks.showMainOnFirstLoad &&
          !this.#quitting &&
          !window.isDestroyed()
        ) {
          window.show();
        }
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

    return window;
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
}
