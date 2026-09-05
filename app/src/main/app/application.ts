import type { HelperClient } from '../helper';
import type { SettingsStore } from '../persistence';
import type { WindowManager } from './window-manager';
import type { DiagnosticLogger } from '../security/diagnostic-logger';
import type { ApplicationLifecycle } from './application-runtime';
import { app } from 'electron';
import { execFileSync } from 'node:child_process';
import { closeSync, constants, lstatSync, openSync, readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { launchWindowsUpdateReadyHelper } from '../info/electron-update-backend';
import { acknowledgeWindowsUpdateAppReady } from '../info/windows-update-relaunch-intent';
import { ApplicationRuntime, type TalkingQuillApplicationOptions } from './application-runtime';
export type { TalkingQuillApplicationOptions } from './application-runtime';
import { ApplicationShutdown } from './application-shutdown';
import { ApplicationReset } from './application-reset';
import { startApplication, type ApplicationStartupHooks } from './application-startup';
import { helperExecutablePath } from './application-helper';
import { WindowRoleRegistry } from './window-role-registry';
export class TalkingQuillApplication {
  // Keep these resources on the facade for the installed-acceptance build overlay.
  #lifecycle: ApplicationLifecycle = 'new';
  #helper: HelperClient | null = null;
  #settings: SettingsStore | null = null;
  #windows: WindowManager | null = null;
  #diagnostics: DiagnosticLogger | null = null;
  readonly #windowsLoginStart: boolean;
  readonly #runtime: ApplicationRuntime;
  readonly #shutdown: ApplicationShutdown;
  readonly #reset: ApplicationReset;
  #applicationActivationSequence = 0;
  readonly #testQuitRequest = () => this.quit();
  readonly #startupHooks: ApplicationStartupHooks = {
    helperExecutablePath,
    resumeMacosCleanup,
    validInstalledMacosOwner,
    acknowledgeWindowsUpdateRelaunches: () => this.#acknowledgePendingWindowsUpdateRelaunches(),
    testQuitRequest: this.#testQuitRequest,
  };

  constructor(options: TalkingQuillApplicationOptions = {}) {
    this.#windowsLoginStart = options.windowsLoginStart === true;
    this.#runtime = new ApplicationRuntime(
      { ...options, windowsLoginStart: this.#windowsLoginStart },
      new WindowRoleRegistry(),
      {
        getLifecycle: () => this.#lifecycle,
        setLifecycle: (value) => {
          this.#lifecycle = value;
        },
        getHelper: () => this.#helper,
        setHelper: (value) => {
          this.#helper = value;
        },
        getSettings: () => this.#settings,
        setSettings: (value) => {
          this.#settings = value;
        },
        getWindows: () => this.#windows,
        setWindows: (value) => {
          this.#windows = value;
        },
        getDiagnostics: () => this.#diagnostics,
        setDiagnostics: (value) => {
          this.#diagnostics = value;
        },
      },
    );
    this.#shutdown = new ApplicationShutdown(this.#runtime, this.#testQuitRequest);
    this.#reset = new ApplicationReset(this.#runtime, this.#shutdown);
  }

  start(): Promise<void> {
    if (this.#runtime.startPromise !== null) return this.#runtime.startPromise;
    if (this.#runtime.lifecycle !== 'new')
      return Promise.reject(new Error('Application cannot start'));
    this.#runtime.lifecycle = 'starting';
    this.#runtime.startPromise = startApplication(
      this.#runtime,
      this.#startupHooks,
      this.#shutdown,
      this.#reset,
    );
    return this.#runtime.startPromise;
  }

  stop(): Promise<void> {
    this.quit();
    return this.#runtime.quitPromise ?? Promise.resolve();
  }

  handleWindowsUpdateRelaunchGeneration(generation: string): void {
    if (!/^[0-9a-f]{32}$/.test(generation)) return;
    this.#runtime.pendingWindowsUpdateRelaunchGenerations.add(generation);
    if (this.#runtime.lifecycle === 'running') this.#acknowledgePendingWindowsUpdateRelaunches();
  }

  #acknowledgePendingWindowsUpdateRelaunches(): void {
    if (process.platform !== 'win32' || !app.isPackaged) return;
    const readyHelper = helperExecutablePath();
    if (readyHelper === null) return;
    for (const generation of this.#runtime.pendingWindowsUpdateRelaunchGenerations) {
      if (this.#runtime.inFlightWindowsUpdateRelaunchGenerations.has(generation)) continue;
      this.#runtime.inFlightWindowsUpdateRelaunchGenerations.add(generation);
      void acknowledgeWindowsUpdateAppReady(
        readyHelper,
        app.getVersion(),
        generation,
        launchWindowsUpdateReadyHelper,
      )
        .then(() => this.#runtime.pendingWindowsUpdateRelaunchGenerations.delete(generation))
        .catch(() => undefined)
        .finally(() => this.#runtime.inFlightWindowsUpdateRelaunchGenerations.delete(generation));
    }
  }

  showMain(): void {
    const windows = this.#runtime.windows;
    if (windows === null) {
      this.#runtime.showMainWhenReady = true;
      return;
    }
    windows.showMainByUser();
  }

  handleApplicationActivation(source: 'second_instance' | 'os_activate'): void {
    this.showMain();
    if (this.#applicationActivationSequence >= 8) return;
    this.#applicationActivationSequence += 1;
    void this.#runtime.diagnostics
      ?.record('application.activation', {
        component: 'application',
        outcome: 'requested',
        activationSource: source,
        activationSequence: this.#applicationActivationSequence,
        restoreHandlerReached: true,
        showMainReached: true,
      })
      .catch(() => undefined);
  }

  quit(deadline?: number): void {
    this.#shutdown.quit(deadline);
  }
  handleBeforeQuit(event: Electron.Event): void {
    this.#shutdown.handleBeforeQuit(event);
  }
  shutdown(): void {
    this.#shutdown.shutdown();
  }
}

function resumeMacosCleanup(helperExecutablePath: string | null): void {
  if (
    app.isPackaged &&
    process.platform === 'darwin' &&
    helperExecutablePath !== null &&
    validInstalledMacosOwner(process.resourcesPath, helperExecutablePath)
  ) {
    // This signed native mode validates pinned CMS cleanup journals and
    // finishes an interrupted committed uninstall before provisioning,
    // updater setup, or any owner connection can run.
    execFileSync(helperExecutablePath, ['--macos-owner-resume-cleanup'], {
      stdio: 'ignore',
      timeout: 30_000,
    });
  }
}

function validInstalledMacosOwner(resourcesPath: string, helperExecutable: string): boolean {
  const marker = join(resourcesPath, 'keyboard-owner-installed-v1');
  const policy = join(resourcesPath, 'keyboard-owner-r5m.json');
  const bridge = join(dirname(resourcesPath), 'MacOS', 'talking-quill-macos-service-bridge');
  try {
    for (const path of [marker, policy, helperExecutable, bridge]) {
      const metadata = lstatSync(path);
      if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) return false;
    }
    const descriptor = openSync(marker, constants.O_RDONLY | constants.O_NOFOLLOW);
    let markerValue: string;
    try {
      markerValue = readFileSync(descriptor, 'utf8');
    } finally {
      closeSync(descriptor);
    }
    if (markerValue !== 'talking-quill-keyboard-owner-v1\n') return false;
    execFileSync(helperExecutable, ['--macos-owner-validate-install'], {
      stdio: 'ignore',
      timeout: 5_000,
      killSignal: 'SIGKILL',
    });
    return true;
  } catch {
    return false;
  }
}
