import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type { EchoSessionController } from '../echo/echo-session-controller';
import type { IpcEventEmitter } from '../ipc/event-emitter';
import type { SettingsStore } from '../persistence/settings-store';
import type { ModelManager, WhisperWorkerClient } from '../transcription';
import type { WelcomeService } from '../welcome/welcome-service';
import type { AppStateService } from './app-state-service';
import { applySelectedModelProgress } from './model-readiness-sync';

/** Coordinates model/worker readiness without owning either resource lifecycle. */
export class ModelRuntimeCoordinator {
  readonly #settings: SettingsStore;
  readonly #events: IpcEventEmitter;
  readonly #models: ModelManager;
  readonly #whisper: WhisperWorkerClient;
  readonly #runtimeValidatedModels = new Set<string>();
  readonly #runtimeValidationTasks = new Map<string, Promise<boolean>>();
  #stateTarget: AppStateService | null = null;
  #echoTarget: EchoSessionController | null = null;
  #welcomeTarget: WelcomeService | null = null;

  constructor(options: {
    readonly settings: SettingsStore;
    readonly events: IpcEventEmitter;
    readonly models: ModelManager;
    readonly whisper: WhisperWorkerClient;
  }) {
    this.#settings = options.settings;
    this.#events = options.events;
    this.#models = options.models;
    this.#whisper = options.whisper;
    this.#models.setBeforeMutation(async (modelId) => {
      this.#runtimeValidatedModels.delete(this.#validationKey(modelId));
      await this.#whisper.unload(modelId);
    });
    this.#models.setAfterInstallValidation(async (modelId, signal) => {
      await this.#whisper.checkWorkerModel(modelId, signal);
      this.#runtimeValidatedModels.add(this.#validationKey(modelId));
    });
  }

  subscribeProgress(): () => void {
    return this.#models.subscribe((progress) => {
      this.#events.send('model:progress', progress);
      if (progress.state !== 'ready') {
        this.#runtimeValidatedModels.delete(this.#validationKey(progress.modelId));
      }
      const selectedModelId = this.#settings.get().transcription.modelId;
      applySelectedModelProgress(progress, selectedModelId, this.#stateTarget, this.#echoTarget);
      if (
        progress.modelId === selectedModelId &&
        progress.state !== 'ready' &&
        this.#settings.get().welcome.modelEvidence != null
      ) {
        void this.#welcomeTarget?.invalidateModelSelection();
      }
    });
  }

  bindState(state: AppStateService): ReturnType<ModelManager['status']> {
    this.#stateTarget = state;
    return this.#models.status(this.#settings.get().transcription.modelId);
  }

  bindEcho(echo: EchoSessionController): () => void {
    this.#echoTarget = echo;
    return () => {
      if (this.#echoTarget === echo) this.#echoTarget = null;
    };
  }

  bindWelcome(welcome: WelcomeService): () => void {
    this.#welcomeTarget = welcome;
    return () => {
      if (this.#welcomeTarget === welcome) this.#welcomeTarget = null;
    };
  }

  async selectedModelReadyForWelcome(): Promise<boolean> {
    const modelId = this.#settings.get().transcription.modelId;
    const key = this.#validationKey(modelId);
    const metadata = await this.#models.status(modelId);
    if (metadata.state !== 'ready') {
      this.#runtimeValidatedModels.delete(key);
      return false;
    }
    if (this.#runtimeValidatedModels.has(key)) return true;
    const existing = this.#runtimeValidationTasks.get(key);
    if (existing !== undefined) return existing;
    const validation = this.#models
      .status(modelId, true)
      .then((status) => {
        if (status.state === 'ready') this.#runtimeValidatedModels.add(key);
        return status.state === 'ready';
      })
      .finally(() => this.#runtimeValidationTasks.delete(key));
    this.#runtimeValidationTasks.set(key, validation);
    return validation;
  }

  manifestRevision(modelId: WhisperModelId): string {
    return this.#models.manifestRevision(modelId);
  }

  subscribeSelectedModel(assumeReady: boolean): () => void {
    let selectedModelId = this.#settings.get().transcription.modelId;
    return this.#settings.subscribe((next) => {
      if (next.transcription.modelId === selectedModelId) return;
      selectedModelId = next.transcription.modelId;
      if (assumeReady) {
        this.#stateTarget?.setModelReady(true);
        this.#echoTarget?.readinessChanged();
        return;
      }
      void this.#models.status(next.transcription.modelId).then((status) => {
        if (this.#settings.get().transcription.modelId === status.modelId) {
          this.#stateTarget?.setModelReady(status.state === 'ready');
          this.#echoTarget?.readinessChanged();
        }
      });
    });
  }

  #validationKey(modelId: WhisperModelId): string {
    return `${modelId}\u0000${this.#models.manifestRevision(modelId)}`;
  }
}
