import type {
  ModelManifestEntry,
  ModelManifestFile,
  WhisperModelId,
} from '../../shared/schemas/model-manifest';
import type { ModelState, ModelStatus } from '../../shared/schemas/transcription';
import type { ModelManifest } from '../../shared/schemas/model-manifest';
import { ModelProgressSchema, type ModelProgress } from '../../shared/schemas/transcription';
import { ModelAccessCoordinator } from './model-access-coordinator';
import { ModelRepository } from './model-repository';
import { MODEL_MANIFEST } from './model-manifest';
import type { ModelManagerOptions } from './model-manager-types';
import { makeStatus, waitForModelTask } from './model-manager-results';

interface StateOverride {
  readonly state: ModelState;
  readonly detail: string;
  readonly repairable: boolean;
}

/** Shared model state, metadata reads, and mutation recovery. */
export class ModelManagerContext {
  readonly manifest: ModelManifest;
  readonly access: ModelAccessCoordinator;
  readonly repository: ModelRepository;
  readonly states = new Map<WhisperModelId, StateOverride>();
  readonly #listeners = new Set<(event: ModelProgress) => void>();
  readonly #recoveryTasks = new Map<WhisperModelId, Promise<void>>();
  beforeMutation: ((modelId: WhisperModelId) => Promise<void>) | null = null;
  afterInstallValidation: ((modelId: WhisperModelId, signal: AbortSignal) => Promise<void>) | null =
    null;

  constructor(options: ModelManagerOptions) {
    this.manifest = options.manifest ?? MODEL_MANIFEST;
    this.access = options.accessCoordinator ?? new ModelAccessCoordinator();
    this.repository = new ModelRepository(options);
  }

  subscribe(listener: (event: ModelProgress) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  manifestRevision(modelId: WhisperModelId): string {
    return this.getModel(modelId).revision;
  }

  async initialize(): Promise<void> {
    await Promise.all(this.manifest.models.map((model) => this.ensureRecovered(model)));
  }

  setBeforeMutation(hook: (modelId: WhisperModelId) => Promise<void>): void {
    this.beforeMutation = hook;
  }

  setAfterInstallValidation(
    hook: (modelId: WhisperModelId, signal: AbortSignal) => Promise<void>,
  ): void {
    this.afterInstallValidation = hook;
  }

  async metadataStatus(model: ModelManifestEntry): Promise<ModelStatus> {
    const override = this.states.get(model.id);
    if (override !== undefined) {
      const inspection = await this.repository.inspectInstalled(model, undefined, false);
      return makeStatus(
        model,
        override.state,
        Math.min(
          inspection.validBytes + (await this.repository.temporaryBytes(model)),
          model.totalBytes,
        ),
        override.detail,
        override.repairable,
      );
    }
    const marker = await this.repository.readCompletionMarker(model);
    if (
      marker.identity !== null &&
      (await this.repository.installedIdentityStillCurrent(model, marker.identity))
    ) {
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    }
    const inspection = await this.repository.inspectInstalled(model, undefined, false);
    if (marker.present && inspection.existingBytes > 0) {
      return makeStatus(
        model,
        'corrupt',
        inspection.validBytes,
        'Model completion identity no longer matches local files.',
        true,
      );
    }
    return makeStatus(
      model,
      inspection.corrupt ? 'corrupt' : 'missing',
      Math.min(
        inspection.validBytes + (await this.repository.temporaryBytes(model)),
        model.totalBytes,
      ),
      inspection.corrupt ? 'Managed model files have invalid metadata.' : null,
      inspection.corrupt,
    );
  }

  async statusFromDisk(
    model: ModelManifestEntry,
    state: ModelState,
    detail: string | null,
    repairable: boolean,
    hash: boolean,
  ): Promise<ModelStatus> {
    const inspection = await this.repository.inspectInstalled(model, undefined, hash);
    return makeStatus(
      model,
      state,
      Math.min(
        inspection.validBytes + (await this.repository.temporaryBytes(model)),
        model.totalBytes,
      ),
      detail,
      repairable,
    );
  }

  async deleteFiles(modelId: WhisperModelId): Promise<void> {
    await this.repository.deleteArtifacts(this.getModel(modelId));
    this.states.delete(modelId);
  }

  async withModelMutation<Value>(
    modelId: WhisperModelId,
    operation: () => Promise<Value>,
    signal?: AbortSignal,
  ): Promise<Value> {
    const model = this.getModel(modelId);
    await waitForModelTask(this.ensureRecovered(model), signal, 'Model mutation was cancelled.');
    return this.access.withMutation(
      modelId,
      async () => {
        await this.recoverAndRemember(model);
        await this.beforeMutation?.(modelId);
        try {
          return await operation();
        } finally {
          await this.recoverAndRemember(model);
        }
      },
      signal,
    );
  }

  ensureRecovered(model: ModelManifestEntry): Promise<void> {
    const existing = this.#recoveryTasks.get(model.id);
    if (existing !== undefined) return existing;
    const recovery = this.access.withMutation(model.id, () =>
      this.repository.recoverArtifacts(model),
    );
    this.#recoveryTasks.set(model.id, recovery);
    void recovery.catch(() => {
      if (this.#recoveryTasks.get(model.id) === recovery) this.#recoveryTasks.delete(model.id);
    });
    return recovery;
  }

  async recoverAndRemember(model: ModelManifestEntry): Promise<void> {
    try {
      await this.repository.recoverArtifacts(model);
      this.#recoveryTasks.set(model.id, Promise.resolve());
    } catch (error: unknown) {
      this.#recoveryTasks.delete(model.id);
      throw error;
    }
  }

  getModel(modelId: WhisperModelId): ModelManifestEntry {
    const model = this.manifest.models.find((candidate) => candidate.id === modelId);
    if (model === undefined) throw new Error(`Unsupported Whisper model: ${modelId}`);
    return model;
  }

  emit(
    model: ModelManifestEntry,
    state: ModelState,
    file: ModelManifestFile | null,
    totalDownloaded: number,
    fileDownloaded = 0,
  ): void {
    const event = ModelProgressSchema.parse({
      modelId: model.id,
      state,
      file:
        file === null
          ? null
          : { path: file.path, downloadedBytes: fileDownloaded, totalBytes: file.size },
      total: {
        downloadedBytes: Math.min(totalDownloaded, model.totalBytes),
        totalBytes: model.totalBytes,
      },
    });
    for (const listener of this.#listeners) {
      try {
        listener(event);
      } catch {
        // Progress observers must not alter model installation state.
      }
    }
  }
}
