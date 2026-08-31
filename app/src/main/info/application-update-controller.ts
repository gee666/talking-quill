import {
  ApplicationUpdateStateSchema,
  type ApplicationUpdateState,
  type UpdateCheckResult,
} from '../../shared/schemas/info';
import { compareVersions, normalizeVersion } from './update-service';
import {
  MacosMaintenancePostponedError,
  type DownloadedApplicationUpdate,
} from './macos-owner-update-coordinator';

export interface ApplicationUpdateBackend {
  checkForUpdates(): Promise<{
    readonly version: string;
    readonly releaseUrl: string | null;
  } | null>;
  downloadUpdate(): Promise<DownloadedApplicationUpdate | undefined>;
  requestElevation?(): Promise<'accepted' | 'cancelled'>;
  quitAndInstall(): void;
  onProgress(listener: (percent: number) => void): () => void;
  onError(listener: () => void): () => void;
  dispose(): void;
}

export interface ApplicationUpdateControllerOptions {
  readonly currentVersion: string;
  readonly backend: ApplicationUpdateBackend | null;
  readonly publish: (state: ApplicationUpdateState) => void;
  readonly requestInstall: () => void;
  readonly prepareInstall?: (download: DownloadedApplicationUpdate) => Promise<void>;
}

/** Owns the consent boundary between release discovery and installer execution. */
export class ApplicationUpdateController {
  readonly #currentVersion: string;
  readonly #backend: ApplicationUpdateBackend | null;
  readonly #publish: (state: ApplicationUpdateState) => void;
  readonly #requestInstall: () => void;
  readonly #prepareInstall: ((download: DownloadedApplicationUpdate) => Promise<void>) | null;
  readonly #removeBackendListeners: (() => void)[] = [];
  #state: ApplicationUpdateState;
  #checking = false;
  #operation: Promise<void> | null = null;
  #downloaded: { readonly version: string; readonly update: DownloadedApplicationUpdate } | null =
    null;
  #disposed = false;

  constructor(options: ApplicationUpdateControllerOptions) {
    this.#currentVersion = normalizeVersion(options.currentVersion);
    this.#backend = options.backend;
    this.#publish = options.publish;
    this.#requestInstall = options.requestInstall;
    this.#prepareInstall = options.prepareInstall ?? null;
    this.#state = ApplicationUpdateStateSchema.parse({
      phase: options.backend === null ? 'unsupported' : 'idle',
      currentVersion: this.#currentVersion,
      availableVersion: null,
      releaseUrl: null,
      latestVersion: null,
      latestReleaseUrl: null,
      percent: null,
      message:
        options.backend === null ? 'This build requires updates to be installed manually.' : null,
      revision: 0,
    });
    if (this.#backend !== null) {
      this.#removeBackendListeners.push(
        this.#backend.onProgress((percent) => this.#handleProgress(percent)),
        this.#backend.onError(() => this.#handleBackendError()),
      );
    }
  }

  getState(): ApplicationUpdateState {
    return this.#state;
  }

  async acceptCheckResult(result: UpdateCheckResult): Promise<ApplicationUpdateState> {
    if (
      this.#disposed ||
      this.#checking ||
      ['downloading', 'installing'].includes(this.#state.phase)
    ) {
      return this.#state;
    }
    if (
      result.status === 'current' ||
      compareVersions(result.latestVersion, this.#currentVersion) <= 0
    ) {
      return this.#setState({
        phase: this.#backend === null ? 'unsupported' : 'current',
        availableVersion: null,
        releaseUrl: null,
        latestVersion: normalizeVersion(result.latestVersion),
        latestReleaseUrl: result.releaseUrl,
        percent: null,
        message:
          this.#backend === null ? 'This build requires updates to be installed manually.' : null,
      });
    }
    const latestVersion = normalizeVersion(result.latestVersion);
    if (this.#backend === null) {
      return this.#setState({
        phase: 'unsupported',
        availableVersion: latestVersion,
        releaseUrl: result.releaseUrl,
        latestVersion,
        latestReleaseUrl: result.releaseUrl,
        percent: null,
        message: 'Install this release manually from its GitHub release page.',
      });
    }
    this.#checking = true;
    try {
      const checked = await this.#backend.checkForUpdates();
      const availableVersion = checked === null ? null : normalizeVersion(checked.version);
      const compatibleReleaseUrl =
        checked?.releaseUrl ?? (availableVersion === latestVersion ? result.releaseUrl : null);
      if (
        this.#isDisposed() ||
        availableVersion === null ||
        compareVersions(availableVersion, this.#currentVersion) <= 0 ||
        compareVersions(availableVersion, latestVersion) > 0 ||
        compatibleReleaseUrl === null
      ) {
        throw new Error('The update metadata did not identify a compatible release edge');
      }
      const state = this.#setState({
        phase: 'available',
        availableVersion,
        releaseUrl: compatibleReleaseUrl,
        latestVersion,
        latestReleaseUrl: result.releaseUrl,
        percent: null,
        message: null,
      });
      if (this.#operation === null) {
        this.#setState({ phase: 'downloading', percent: 0, message: null });
        this.#operation = this.#downloadAutomatically(availableVersion).finally(() => {
          this.#operation = null;
        });
      }
      return state;
    } catch {
      return this.#setState({
        phase: 'error',
        availableVersion: null,
        releaseUrl: null,
        latestVersion,
        latestReleaseUrl: result.releaseUrl,
        percent: null,
        message: 'The update metadata did not identify a compatible release edge.',
      });
    } finally {
      this.#checking = false;
    }
  }

  apply(): ApplicationUpdateState {
    if (
      this.#disposed ||
      this.#backend === null ||
      this.#state.phase !== 'available' ||
      this.#state.availableVersion === null ||
      this.#operation !== null
    ) {
      return this.#state;
    }
    const expectedVersion = this.#state.availableVersion;
    this.#setState({ phase: 'installing', percent: 100, message: null });
    this.#operation = this.#prepareAndInstall(expectedVersion).finally(() => {
      this.#operation = null;
    });
    return this.#state;
  }

  quitAndInstall(): void {
    if (this.#state.phase !== 'installing' || this.#backend === null) {
      throw new Error('No verified application update is ready to install');
    }
    this.#backend.quitAndInstall();
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    for (const remove of this.#removeBackendListeners.splice(0)) remove();
    this.#backend?.dispose();
  }

  async #downloadAutomatically(expectedVersion: string): Promise<void> {
    try {
      const backend = this.#backend;
      if (backend === null) throw new Error('The update backend is unavailable');
      const update = await backend.downloadUpdate();
      if (this.#isDisposed()) return;
      if (update === undefined) throw new Error('The updater did not identify its candidate');
      this.#downloaded = { version: expectedVersion, update };
      this.#setState({
        phase: 'available',
        availableVersion: expectedVersion,
        percent: null,
        message: null,
      });
    } catch {
      if (!this.#disposed) {
        this.#downloaded = null;
        this.#setState({
          phase: 'error',
          percent: null,
          message: 'The update could not be downloaded. Try again or use the release page.',
        });
      }
    }
  }

  async #prepareAndInstall(expectedVersion: string): Promise<void> {
    try {
      const downloaded = this.#downloaded;
      if (downloaded?.version !== expectedVersion) {
        throw new Error('The exact downloaded update identity is unavailable');
      }
      await this.#prepareInstall?.(downloaded.update);
      if (!this.#installationStillActive()) return;
      const elevation = await this.#backend?.requestElevation?.();
      if (!this.#installationStillActive()) return;
      if (elevation === 'cancelled') {
        this.#setState({
          phase: 'available',
          percent: null,
          message: 'Administrator approval was cancelled. The update remains available.',
        });
        return;
      }
      this.#requestInstall();
    } catch (error: unknown) {
      if (!this.#disposed) {
        this.#setState({
          phase: 'error',
          percent: null,
          message:
            error instanceof MacosMaintenancePostponedError
              ? error.message
              : 'The update could not be prepared. Try again or use the release page.',
        });
      }
    }
  }

  #isDisposed(): boolean {
    return this.#disposed;
  }

  #installationStillActive(): boolean {
    return !this.#disposed && this.#state.phase === 'installing';
  }

  #handleProgress(value: number): void {
    if (this.#disposed || this.#state.phase !== 'downloading' || !Number.isFinite(value)) return;
    const percent = Math.min(100, Math.max(0, value));
    if (this.#state.percent !== null && Math.abs(percent - this.#state.percent) < 0.5) return;
    this.#setState({ percent });
  }

  #handleBackendError(): void {
    if (this.#disposed || this.#state.phase === 'idle' || this.#state.phase === 'current') return;
    this.#setState({
      phase: 'error',
      percent: null,
      message: 'The update service reported an error. Try again or use the release page.',
    });
  }

  #setState(patch: Partial<ApplicationUpdateState>): ApplicationUpdateState {
    this.#state = ApplicationUpdateStateSchema.parse({
      ...this.#state,
      ...patch,
      revision: this.#state.revision + 1,
    });
    this.#publish(this.#state);
    return this.#state;
  }
}
