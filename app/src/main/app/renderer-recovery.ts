import type { BrowserWindow } from 'electron';
import type { WindowRole } from '../../shared/constants/app';
import type { RendererLoader } from './renderer-loader';
import type { WindowRoleRegistry } from './window-role-registry';

const MAX_RENDERER_RECOVERY_ATTEMPTS = 2;
const RENDERER_RECOVERY_BACKOFF_MS = 250;
const RENDERER_STABILITY_WINDOW_MS = 30_000;
export const RENDERER_LOAD_TIMEOUT_MS = 10_000;

interface RendererRecoveryOptions {
  readonly windows: Map<WindowRole, BrowserWindow>;
  readonly roles: WindowRoleRegistry;
  readonly loader: RendererLoader;
  readonly requestQuit: () => void;
  readonly onMainFailed: () => void;
  readonly createAndLoad: (role: WindowRole) => Promise<boolean>;
}

export class RendererRecovery {
  readonly #recoveryAttempts = new Map<WindowRole, number>();
  readonly #recoveryTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  readonly #stabilityTimers = new Map<WindowRole, ReturnType<typeof setTimeout>>();
  readonly #pendingRendererLoads = new Map<BrowserWindow, () => void>();
  readonly #rendererReadyWebContents = new Set<number>();
  readonly #pendingRendererReady = new Map<number, (ready: boolean) => void>();
  #stopped = false;
  readonly #options: RendererRecoveryOptions;

  constructor(options: RendererRecoveryOptions) {
    this.#options = options;
  }

  markReady(webContentsId: number): void {
    this.#rendererReadyWebContents.add(webContentsId);
    this.#pendingRendererReady.get(webContentsId)?.(true);
  }
  loadRenderer(
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
        void this.#options.loader.load(window, role).then(
          () => finish('loaded'),
          () => finish('failed'),
        );
      } catch {
        finish('failed');
      }
    });
  }

  waitForRendererReady(window: BrowserWindow): Promise<boolean> {
    const webContentsId = window.webContents.id;
    if (this.#rendererReadyWebContents.has(webContentsId)) return Promise.resolve(true);
    return new Promise((resolve) => {
      let finished = false;
      const finish = (ready: boolean): void => {
        if (finished) return;
        finished = true;
        clearTimeout(timer);
        if (this.#pendingRendererReady.get(webContentsId) === finish) {
          this.#pendingRendererReady.delete(webContentsId);
        }
        resolve(ready);
      };
      const timer = setTimeout(() => finish(false), RENDERER_LOAD_TIMEOUT_MS);
      timer.unref();
      this.#pendingRendererReady.set(webContentsId, finish);
      if (this.#rendererReadyWebContents.has(webContentsId)) finish(true);
    });
  }

  forgetRendererReady(window: BrowserWindow): void {
    const webContentsId = window.webContents.id;
    this.#rendererReadyWebContents.delete(webContentsId);
    this.#pendingRendererReady.get(webContentsId)?.(false);
  }

  attachRecovery(window: BrowserWindow, role: WindowRole): void {
    window.webContents.once('did-finish-load', () => {
      if (this.#stopped || this.#options.windows.get(role) !== window) return;
      this.#clearRoleTimer(this.#stabilityTimers, role);
      const timer = setTimeout(() => {
        if (this.#stabilityTimers.get(role) !== timer) return;
        this.#stabilityTimers.delete(role);
        if (!this.#stopped && this.#options.windows.get(role) === window) {
          this.#recoveryAttempts.delete(role);
        }
      }, RENDERER_STABILITY_WINDOW_MS);
      this.#stabilityTimers.set(role, timer);
      timer.unref();
    });
    window.webContents.on('did-fail-load', (_event, errorCode) => {
      if (errorCode !== -3) this.recover(role, window);
    });
    window.webContents.on('render-process-gone', () => this.recover(role, window));
    window.on('unresponsive', () => {
      if (role === 'widget') this.recover(role, window);
    });
  }

  recover(role: WindowRole, failed: BrowserWindow): void {
    if (this.#stopped || this.#options.windows.get(role) !== failed) return;
    if (role === 'main') this.#options.onMainFailed();
    this.#pendingRendererLoads.get(failed)?.();
    this.forgetRendererReady(failed);
    this.#clearRoleTimer(this.#stabilityTimers, role);
    const attempts = (this.#recoveryAttempts.get(role) ?? 0) + 1;
    this.#recoveryAttempts.set(role, attempts);
    this.#options.windows.delete(role);
    this.#options.roles.unregister(failed.webContents.id);
    if (!failed.isDestroyed()) failed.destroy();
    if (attempts > MAX_RENDERER_RECOVERY_ATTEMPTS) {
      this.#options.requestQuit();
      return;
    }
    this.#clearRoleTimer(this.#recoveryTimers, role);
    const timer = setTimeout(() => {
      if (this.#recoveryTimers.get(role) !== timer) return;
      this.#recoveryTimers.delete(role);
      if (this.#stopped) return;
      void this.#options.createAndLoad(role).catch(() => undefined);
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
  invalidate(window: BrowserWindow): void {
    this.#pendingRendererLoads.get(window)?.();
  }

  resetRole(role: WindowRole): void {
    this.#clearRoleTimer(this.#recoveryTimers, role);
    this.#clearRoleTimer(this.#stabilityTimers, role);
    this.#recoveryAttempts.delete(role);
  }

  stop(): void {
    this.#stopped = true;
    for (const invalidate of [...this.#pendingRendererLoads.values()]) invalidate();
    for (const complete of [...this.#pendingRendererReady.values()]) complete(false);
    this.#clearTimers(this.#recoveryTimers);
    this.#clearTimers(this.#stabilityTimers);
  }

  clearReady(): void {
    this.#rendererReadyWebContents.clear();
  }
}
