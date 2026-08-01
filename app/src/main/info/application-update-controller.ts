import {
  ApplicationUpdateStateSchema,
  type ApplicationUpdateState,
  type UpdateCheckResult,
} from '../../shared/schemas/info';
import { compareVersions, normalizeVersion } from './update-service';

export interface ApplicationUpdateBackend {
  checkForUpdates(): Promise<{ readonly version: string } | null>;
  downloadUpdate(): Promise<void>;
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
}

/** Owns the consent boundary between release discovery and installer execution. */
export class ApplicationUpdateController {
  readonly #currentVersion: string;
  readonly #backend: ApplicationUpdateBackend | null;
  readonly #publish: (state: ApplicationUpdateState) => void;
  readonly #requestInstall: () => void;
  readonly #removeBackendListeners: (() => void)[] = [];
  #state: ApplicationUpdateState;
  #operation: Promise<void> | null = null;
  #disposed = false;

  constructor(options: ApplicationUpdateControllerOptions) {
    this.#currentVersion = normalizeVersion(options.currentVersion);
    this.#backend = options.backend;
    this.#publish = options.publish;
    this.#requestInstall = options.requestInstall;
    this.#state = ApplicationUpdateStateSchema.parse({
      phase: options.backend === null ? 'unsupported' : 'idle',
      currentVersion: this.#currentVersion,
      availableVersion: null,
      releaseUrl: null,
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

  acceptCheckResult(result: UpdateCheckResult): ApplicationUpdateState {
    if (this.#disposed || ['downloading', 'installing'].includes(this.#state.phase)) {
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
        percent: null,
        message:
          this.#backend === null ? 'This build requires updates to be installed manually.' : null,
      });
    }
    return this.#setState({
      phase: this.#backend === null ? 'unsupported' : 'available',
      availableVersion: normalizeVersion(result.latestVersion),
      releaseUrl: result.releaseUrl,
      percent: null,
      message:
        this.#backend === null
          ? 'Install this release manually from its GitHub release page.'
          : null,
    });
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
    this.#setState({ phase: 'downloading', percent: 0, message: null });
    this.#operation = this.#downloadAndInstall(expectedVersion).finally(() => {
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

  async #downloadAndInstall(expectedVersion: string): Promise<void> {
    try {
      const backend = this.#backend;
      if (backend === null) throw new Error('The update backend is unavailable');
      const update = await backend.checkForUpdates();
      if (
        this.#disposed ||
        update === null ||
        normalizeVersion(update.version) !== expectedVersion
      ) {
        throw new Error('The update metadata did not match the selected release');
      }
      await backend.downloadUpdate();
      if (this.#isDisposed()) return;
      this.#setState({ phase: 'installing', percent: 100, message: null });
      this.#requestInstall();
    } catch {
      if (!this.#disposed) {
        this.#setState({
          phase: 'error',
          percent: null,
          message: 'The update could not be downloaded. Try again or use the release page.',
        });
      }
    }
  }

  #isDisposed(): boolean {
    return this.#disposed;
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
