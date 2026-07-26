import { dialog, type BrowserWindow, type OpenDialogOptions } from 'electron';
import { posix, win32 } from 'node:path';
import type { PiInstallationStatus } from '../../shared/schemas/pi-installation';
import { PiInstallationPathSchema } from '../../shared/schemas/settings';
import type { SettingsStore } from '../persistence/settings-store';
import { piInstallationStatus } from './pi-discovery';
import { ProviderError } from './errors';

export interface PiInstallationDialogs {
  readonly choose: (owner: BrowserWindow, options: OpenDialogOptions) => Promise<string | null>;
}

const DEFAULT_DIALOGS: PiInstallationDialogs = Object.freeze({
  choose: async (owner: BrowserWindow, options: OpenDialogOptions) => {
    const result = await dialog.showOpenDialog(owner, options);
    return result.canceled ? null : (result.filePaths[0] ?? null);
  },
});

const PI_INSTALLATION_OPERATION_TIMEOUT_MS = 120_000;
const PI_INSTALLATION_CLEANUP_RESERVE_MS = 5_000;

export class PiInstallationService {
  readonly #settings: SettingsStore;
  readonly #environment: NodeJS.ProcessEnv;
  readonly #platform: NodeJS.Platform;
  readonly #dialogs: PiInstallationDialogs;
  readonly #interactiveAppData: string | undefined;
  readonly #statusProbe: (
    configuredPath: string | null,
    signal: AbortSignal,
  ) => Promise<PiInstallationStatus>;

  constructor(
    settings: SettingsStore,
    options: {
      readonly environment?: NodeJS.ProcessEnv;
      readonly platform?: NodeJS.Platform;
      readonly dialogs?: PiInstallationDialogs;
      readonly statusProbe?: (
        configuredPath: string | null,
        signal: AbortSignal,
      ) => Promise<PiInstallationStatus>;
      /** Electron's interactive-user appData known folder; survives packaged restricted PATH/env. */
      readonly interactiveAppData?: string;
      readonly interactiveHome?: string;
    } = {},
  ) {
    this.#settings = settings;
    this.#platform = options.platform ?? process.platform;
    const inherited = options.environment ?? process.env;
    this.#environment =
      this.#platform === 'win32' &&
      options.interactiveHome !== undefined &&
      win32.isAbsolute(options.interactiveHome)
        ? Object.fromEntries([
            ...Object.entries(inherited).filter(([key]) => key.toLowerCase() !== 'userprofile'),
            ['USERPROFILE', options.interactiveHome],
          ])
        : inherited;
    this.#dialogs = options.dialogs ?? DEFAULT_DIALOGS;
    this.#interactiveAppData = options.interactiveAppData;
    this.#statusProbe =
      options.statusProbe ??
      ((configuredPath, signal) =>
        piInstallationStatus(
          this.#environment,
          this.#platform,
          configuredPath,
          this.#interactiveAppData,
          signal,
        ));
  }

  configuredPath(): string | null {
    return this.#settings.get().smartProcessing.piInstallationPath;
  }

  async status(): Promise<PiInstallationStatus> {
    const controller = new AbortController();
    return await this.#status(Date.now() + PI_INSTALLATION_OPERATION_TIMEOUT_MS, controller);
  }

  async #status(
    deadline: number,
    controller: AbortController,
    configuredPath = this.configuredPath(),
  ): Promise<PiInstallationStatus> {
    return await withinInstallationDeadline(
      this.#statusProbe(configuredPath, controller.signal),
      deadline,
      () => controller.abort(),
    );
  }

  async save(pathInput: string | null): Promise<PiInstallationStatus> {
    const controller = new AbortController();
    return await this.#save(
      pathInput,
      Date.now() + PI_INSTALLATION_OPERATION_TIMEOUT_MS,
      controller,
    );
  }

  async #save(
    pathInput: string | null,
    deadline: number,
    controller: AbortController,
  ): Promise<PiInstallationStatus> {
    const path = PiInstallationPathSchema.parse(pathInput);
    if (path !== null && !(this.#platform === 'win32' ? win32 : posix).isAbsolute(path)) {
      throw new ProviderError('PI_CONFIG_INVALID');
    }
    // Finish every fallible discovery/capability probe before committing the path. A reported
    // timeout or validation error must never be followed by a late persisted setting.
    const status = await this.#status(deadline, controller, path);
    if (path !== null && status.state !== 'ready') {
      throw new ProviderError(status.errorCode ?? 'PI_CONFIG_INVALID');
    }
    await withinInstallationDeadline(
      this.#settings.update({ smartProcessing: { piInstallationPath: path } }, controller.signal),
      deadline,
      () => controller.abort(),
    );
    return status;
  }

  async browse(owner: BrowserWindow): Promise<string | null> {
    // Keep unbounded human dialog dwell separate from bounded validation and persistence. The
    // renderer explicitly submits a non-null selection through save() after this returns.
    return await this.#dialogs.choose(owner, {
      title: 'Choose Pi installation folder',
      buttonLabel: 'Use this folder',
      properties: ['openDirectory'],
    });
  }
}

function withinInstallationDeadline<Result>(
  operation: Promise<Result>,
  deadline: number,
  abort: () => void = () => undefined,
): Promise<Result> {
  const remaining = deadline - Date.now();
  if (remaining <= 0) return Promise.reject(new ProviderError('TIMEOUT'));
  return new Promise<Result>((resolveOperation, rejectOperation) => {
    let expired = false;
    const abortAfter = Math.max(1, remaining - PI_INSTALLATION_CLEANUP_RESERVE_MS);
    const timer = setTimeout(() => {
      expired = true;
      abort();
    }, abortAfter);
    void operation.then(
      (result) => {
        clearTimeout(timer);
        if (expired) rejectOperation(new ProviderError('TIMEOUT'));
        else resolveOperation(result);
      },
      (error: unknown) => {
        clearTimeout(timer);
        if (expired) rejectOperation(new ProviderError('TIMEOUT'));
        else rejectOperation(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
