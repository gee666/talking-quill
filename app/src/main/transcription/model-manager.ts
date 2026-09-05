import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import type {
  ModelDeleteResult,
  ModelProgress,
  ModelStatus,
} from '../../shared/schemas/transcription';
import { ModelManagerError } from './errors';
import { ModelManagerContext } from './model-manager-context';
import { ModelManagerInstallation } from './model-manager-installation';
import { ModelManagerVerification } from './model-manager-verification';
import type {
  ActiveDownload,
  DownloadIntent,
  ModelManagerOptions,
  ModelUseGrant,
} from './model-manager-types';
import { makeStatus, shuttingDownError, waitForModelTask } from './model-manager-results';

export type { ModelManagerOptions, ModelUseGrant } from './model-manager-types';

/** Public lifecycle and concurrency facade for model installation and use. */
export class ModelManager {
  readonly #context: ModelManagerContext;
  readonly #installation: ModelManagerInstallation;
  readonly #verification: ModelManagerVerification;
  #active: ActiveDownload | null = null;
  #shuttingDown = false;

  constructor(options: ModelManagerOptions) {
    this.#context = new ModelManagerContext(options);
    this.#installation = new ModelManagerInstallation(this.#context, options);
    this.#verification = new ModelManagerVerification(this.#context);
  }

  subscribe(listener: (event: ModelProgress) => void): () => void {
    return this.#context.subscribe(listener);
  }

  manifestRevision(modelId: WhisperModelId): string {
    return this.#context.manifestRevision(modelId);
  }

  initialize(): Promise<void> {
    return this.#context.initialize();
  }

  setBeforeMutation(hook: (modelId: WhisperModelId) => Promise<void>): void {
    this.#context.setBeforeMutation(hook);
  }

  setAfterInstallValidation(
    hook: (modelId: WhisperModelId, signal: AbortSignal) => Promise<void>,
  ): void {
    this.#context.setAfterInstallValidation(hook);
  }

  async list(verify = false): Promise<ModelStatus[]> {
    return Promise.all(this.#context.manifest.models.map((model) => this.status(model.id, verify)));
  }

  async status(modelId: WhisperModelId, verify = false): Promise<ModelStatus> {
    if (verify && this.#shuttingDown) throw shuttingDownError();
    const model = this.#context.getModel(modelId);
    await this.#context.ensureRecovered(model);
    if (this.#active?.modelId === modelId) {
      return this.#context.statusFromDisk(model, this.#active.state, null, false, false);
    }
    if (!verify) return this.#context.metadataStatus(model);
    return this.#verification.verify(model);
  }

  async verifyForUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    if (this.#shuttingDown) throw shuttingDownError();
    const model = this.#context.getModel(modelId);
    await this.#context.ensureRecovered(model);
    return this.#verification.verify(model, signal);
  }

  async acquireUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelUseGrant> {
    if (this.#shuttingDown) throw shuttingDownError();
    const model = this.#context.getModel(modelId);
    await this.#context.ensureRecovered(model);
    let lease = await this.#context.access.acquireUse(modelId, signal);
    try {
      let status = await this.#context.metadataStatus(model);
      const marker = await this.#context.repository.readCompletionMarker(model);
      if (
        !marker.present &&
        status.state === 'missing' &&
        status.downloadedBytes === model.totalBytes
      ) {
        lease.release();
        const verified = await this.#verification.verify(model, signal);
        lease = await this.#context.access.acquireUse(modelId, signal);
        status = await this.#context.metadataStatus(model);
        if (
          status.state !== 'ready' &&
          status.downloadedBytes === model.totalBytes &&
          verified.state === 'corrupt'
        ) {
          status = verified;
        }
      }
      return { status, release: () => lease.release() };
    } catch (error: unknown) {
      lease.release();
      throw error;
    }
  }

  download(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    if (this.#shuttingDown) {
      return Promise.reject(new ModelManagerError('CANCELLED', 'Model manager is shutting down.'));
    }
    if (this.#active !== null) {
      return this.#active.modelId === modelId
        ? waitForModelTask(this.#active.settled, signal, 'Model download was cancelled.')
        : Promise.reject(new ModelManagerError('BUSY', 'Another model download is active.'));
    }
    const controller = new AbortController();
    const active: ActiveDownload = {
      modelId,
      controller,
      intent: 'running',
      state: 'downloading',
      settled: Promise.resolve(
        makeStatus(this.#context.getModel(modelId), 'missing', 0, null, false),
      ),
    };
    const onExternalAbort = () => {
      active.intent = 'external';
      controller.abort(signal?.reason);
    };
    if (signal?.aborted === true) onExternalAbort();
    else signal?.addEventListener('abort', onExternalAbort, { once: true });
    const settled = this.#context
      .withModelMutation(
        modelId,
        () => this.#installation.download(modelId, active),
        active.controller.signal,
      )
      .catch((error: unknown) => {
        if (
          active.controller.signal.aborted &&
          error instanceof ModelManagerError &&
          error.code === 'CANCELLED'
        ) {
          return this.#installation.finishCancelledDownload(
            this.#context.getModel(modelId),
            active,
          );
        }
        throw error;
      })
      .finally(() => {
        signal?.removeEventListener('abort', onExternalAbort);
        if (this.#active === active) this.#active = null;
      });
    Object.defineProperty(active, 'settled', { value: settled });
    this.#active = active;
    return settled;
  }

  async pause(modelId: WhisperModelId): Promise<ModelStatus> {
    const active = this.#active;
    if (active?.modelId !== modelId) return this.status(modelId);
    active.intent = 'paused';
    active.controller.abort('paused');
    return active.settled;
  }

  async cancel(modelId: WhisperModelId): Promise<ModelStatus> {
    const active = this.#active;
    if (active === null && (await this.status(modelId)).state === 'ready') {
      return this.status(modelId);
    }
    await this.#abortActive(modelId, 'cancelled');
    await this.#context.withModelMutation(modelId, async () => {
      await this.#context.repository.removeTemporaryRevision(this.#context.getModel(modelId));
      this.#context.states.delete(modelId);
    });
    return this.status(modelId);
  }

  retry(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    return this.download(modelId, signal);
  }

  async delete(modelId: WhisperModelId): Promise<ModelStatus> {
    await this.#abortActive(modelId, 'cancelled');
    await this.#context.withModelMutation(modelId, () => this.#context.deleteFiles(modelId));
    const status = await this.status(modelId);
    this.#context.emit(this.#context.getModel(modelId), status.state, null, status.downloadedBytes);
    return status;
  }

  async deleteIfIdle(modelId: WhisperModelId): Promise<ModelDeleteResult> {
    const model = this.#context.getModel(modelId);
    const lease = this.#context.access.tryAcquireMutation(modelId);
    if (lease === null) {
      return { outcome: 'in-use', status: await this.status(modelId) };
    }
    try {
      await this.#context.recoverAndRemember(model);
      await this.#context.beforeMutation?.(modelId);
      await this.#context.deleteFiles(modelId);
    } finally {
      try {
        await this.#context.recoverAndRemember(model);
      } finally {
        lease.release();
      }
    }
    const status = await this.status(modelId);
    this.#context.emit(model, status.state, null, status.downloadedBytes);
    return { outcome: 'deleted', status };
  }

  async shutdown(): Promise<void> {
    this.#shuttingDown = true;
    const active = this.#active;
    if (active !== null) {
      active.intent = 'shutdown';
      active.controller.abort('shutdown');
    }
    const verificationLifecycle = this.#verification.abortForShutdown();
    await Promise.allSettled([
      ...(active === null ? [] : [active.settled]),
      ...verificationLifecycle,
    ]);
  }

  async #abortActive(modelId: WhisperModelId, intent: DownloadIntent): Promise<void> {
    const active = this.#active;
    if (active?.modelId !== modelId) return;
    active.intent = intent;
    active.controller.abort(intent);
    await active.settled.catch(() => undefined);
  }
}
