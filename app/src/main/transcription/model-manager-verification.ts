import type { ModelManifestEntry, WhisperModelId } from '../../shared/schemas/model-manifest';
import type { ModelStatus } from '../../shared/schemas/transcription';
import { ModelManagerError } from './errors';
import type { ModelManagerContext } from './model-manager-context';
import { makeStatus, mapDownloadError } from './model-manager-results';

interface SharedVerification {
  readonly controller: AbortController;
  readonly promise: Promise<ModelStatus>;
  readonly waiters: Set<symbol>;
  settled: boolean;
}

/** Shares verification work while keeping cancellation local to each waiter. */
export class ModelManagerVerification {
  readonly #context: ModelManagerContext;
  readonly #verificationTasks = new Map<WhisperModelId, SharedVerification>();
  readonly #verificationLifecycle = new Set<SharedVerification>();

  constructor(context: ModelManagerContext) {
    this.#context = context;
  }

  abortForShutdown(): Promise<ModelStatus>[] {
    const lifecycle = [...this.#verificationLifecycle];
    for (const verification of lifecycle) verification.controller.abort('shutdown');
    return lifecycle.map((verification) => verification.promise);
  }

  verify(model: ModelManifestEntry, signal?: AbortSignal): Promise<ModelStatus> {
    if (signal?.aborted === true) {
      return Promise.reject(
        new ModelManagerError('CANCELLED', 'Model verification was cancelled.'),
      );
    }
    let verification = this.#verificationTasks.get(model.id);
    if (verification === undefined) {
      const controller = new AbortController();
      const promise = this.#runAuthoritativeVerification(model, controller.signal);
      verification = { controller, promise, waiters: new Set(), settled: false };
      this.#verificationTasks.set(model.id, verification);
      this.#verificationLifecycle.add(verification);
      const current = verification;
      void promise.then(
        () => this.#finishSharedVerification(model.id, current),
        () => this.#finishSharedVerification(model.id, current),
      );
    }
    return this.#waitForSharedVerification(model.id, verification, signal);
  }

  async #runAuthoritativeVerification(
    model: ModelManifestEntry,
    signal: AbortSignal,
  ): Promise<ModelStatus> {
    const taskLease = await this.#context.access.acquireUse(model.id, signal);
    try {
      const inspection = await this.#context.repository.inspectInstalled(model, signal, true);
      signal.throwIfAborted();
      if (inspection.valid) {
        try {
          await this.#context.afterInstallValidation?.(model.id, signal);
          signal.throwIfAborted();
          await this.#context.repository.commitVerification(model, inspection.identities);
        } catch (error: unknown) {
          const mapped = mapDownloadError(error, 'verifying');
          if (mapped.code === 'CANCELLED') throw mapped;
          await this.#context.repository.removeCompletionMarker(model).catch(() => undefined);
          this.#context.states.set(model.id, {
            state: 'error',
            detail: mapped.message,
            repairable: mapped.repairable,
          });
          return makeStatus(model, 'error', model.totalBytes, mapped.message, mapped.repairable);
        }
        this.#context.states.delete(model.id);
        return makeStatus(model, 'ready', model.totalBytes, null, false);
      }
      await this.#context.repository.removeCompletionMarker(model);
      if (inspection.existingBytes > 0) {
        return makeStatus(
          model,
          'corrupt',
          inspection.validBytes,
          'Model files failed checksum verification.',
          true,
        );
      }
      return makeStatus(model, 'missing', 0, 'The selected model is not installed.', false);
    } finally {
      taskLease.release();
    }
  }

  #waitForSharedVerification(
    modelId: WhisperModelId,
    verification: SharedVerification,
    signal?: AbortSignal,
  ): Promise<ModelStatus> {
    const waiter = Symbol('model-verification-waiter');
    verification.waiters.add(waiter);
    return new Promise((resolve, reject) => {
      let finished = false;
      const finish = (operation: () => void) => {
        if (finished) return;
        finished = true;
        signal?.removeEventListener('abort', onAbort);
        verification.waiters.delete(waiter);
        if (verification.waiters.size === 0 && !verification.settled) {
          if (this.#verificationTasks.get(modelId) === verification) {
            this.#verificationTasks.delete(modelId);
          }
          verification.controller.abort('no verification waiters remain');
        }
        operation();
      };
      const onAbort = () =>
        finish(() =>
          reject(new ModelManagerError('CANCELLED', 'Model verification was cancelled.')),
        );
      signal?.addEventListener('abort', onAbort, { once: true });
      void verification.promise.then(
        (status) => finish(() => resolve(status)),
        (error: unknown) =>
          finish(() =>
            reject(
              verification.controller.signal.aborted
                ? new ModelManagerError('CANCELLED', 'Model verification was cancelled.')
                : error instanceof Error
                  ? error
                  : new ModelManagerError('IO', 'Model verification failed.', true),
            ),
          ),
      );
    });
  }

  #finishSharedVerification(modelId: WhisperModelId, verification: SharedVerification): void {
    verification.settled = true;
    this.#verificationLifecycle.delete(verification);
    if (this.#verificationTasks.get(modelId) === verification) {
      this.#verificationTasks.delete(modelId);
    }
  }
}
