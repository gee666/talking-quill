import type { link, rename } from 'node:fs/promises';
import type {
  ModelManifest,
  ModelManifestEntry,
  ModelManifestFile,
  WhisperModelId,
  VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import {
  ModelProgressSchema,
  ModelStatusSchema,
  type ModelDeleteResult,
  type ModelProgress,
  type ModelState,
  type ModelStatus,
} from '../../shared/schemas/transcription';
import type { EgressObserver } from '../security/egress-audit';
import { ModelAccessCoordinator } from './model-access-coordinator';
import { ModelDownloadTransport, type ModelDownloadResponse } from './model-download-transport';
import { ModelManagerError, WhisperClientError } from './errors';
import type { inspectFile } from './model-integrity';
import { MODEL_MANIFEST } from './model-manifest';
import type { RevisionBackupRemover } from './model-publication';
import { ModelRepository } from './model-repository';

type DownloadIntent = 'running' | 'paused' | 'cancelled' | 'external' | 'shutdown';

interface ActiveDownload {
  readonly modelId: WhisperModelId;
  readonly controller: AbortController;
  readonly settled: Promise<ModelStatus>;
  intent: DownloadIntent;
  state: Extract<ModelState, 'downloading' | 'verifying' | 'installing'>;
}

interface StateOverride {
  readonly state: ModelState;
  readonly detail: string;
  readonly repairable: boolean;
}

interface SharedVerification {
  readonly controller: AbortController;
  readonly promise: Promise<ModelStatus>;
  readonly waiters: Set<symbol>;
  settled: boolean;
}

export interface ModelUseGrant {
  readonly status: ModelStatus;
  release(): void;
}

export interface ModelManagerOptions {
  readonly modelsDirectory: string;
  readonly temporaryDirectory: string;
  readonly fetch?: typeof fetch;
  readonly availableBytes?: (path: string) => Promise<number>;
  readonly urlFor?: (model: ModelManifestEntry, file: ModelManifestFile) => string;
  readonly validateRequestUrl?: (url: string) => boolean;
  readonly requestTimeoutMs?: number;
  readonly manifest?: ModelManifest;
  readonly accessCoordinator?: ModelAccessCoordinator;
  readonly inspectFile?: typeof inspectFile;
  readonly observeEgress?: EgressObserver;
  /** Test seams for platform filesystem behavior; production uses node:fs/promises. */
  readonly rename?: typeof rename;
  readonly link?: typeof link;
  readonly removeRevisionBackup?: RevisionBackupRemover;
}

/** Public lifecycle and concurrency facade for model installation and use. */
export class ModelManager {
  readonly #manifest: ModelManifest;
  readonly #access: ModelAccessCoordinator;
  readonly #transport: ModelDownloadTransport;
  readonly #repository: ModelRepository;
  readonly #listeners = new Set<(event: ModelProgress) => void>();
  readonly #states = new Map<WhisperModelId, StateOverride>();
  readonly #verificationTasks = new Map<WhisperModelId, SharedVerification>();
  readonly #verificationLifecycle = new Set<SharedVerification>();
  readonly #recoveryTasks = new Map<WhisperModelId, Promise<void>>();
  #beforeMutation: ((modelId: WhisperModelId) => Promise<void>) | null = null;
  #afterInstallValidation:
    ((modelId: WhisperModelId, signal: AbortSignal) => Promise<void>) | null = null;
  #active: ActiveDownload | null = null;
  #shuttingDown = false;

  constructor(options: ModelManagerOptions) {
    this.#manifest = options.manifest ?? MODEL_MANIFEST;
    this.#access = options.accessCoordinator ?? new ModelAccessCoordinator();
    this.#transport = new ModelDownloadTransport(options);
    this.#repository = new ModelRepository(options);
  }

  subscribe(listener: (event: ModelProgress) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  manifestRevision(modelId: WhisperModelId): string {
    return this.#getModel(modelId).revision;
  }

  async initialize(): Promise<void> {
    await Promise.all(this.#manifest.models.map((model) => this.#ensureRecovered(model)));
  }

  setBeforeMutation(hook: (modelId: WhisperModelId) => Promise<void>): void {
    this.#beforeMutation = hook;
  }

  setAfterInstallValidation(
    hook: (modelId: WhisperModelId, signal: AbortSignal) => Promise<void>,
  ): void {
    this.#afterInstallValidation = hook;
  }

  async list(verify = false): Promise<ModelStatus[]> {
    return Promise.all(this.#manifest.models.map((model) => this.status(model.id, verify)));
  }

  async status(modelId: WhisperModelId, verify = false): Promise<ModelStatus> {
    if (verify && this.#shuttingDown) throw shuttingDownError();
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    if (this.#active?.modelId === modelId) {
      return this.#statusFromDisk(model, this.#active.state, null, false, false);
    }
    if (!verify) return this.#metadataStatus(model);
    return this.#verifyAuthoritatively(model);
  }

  async verifyForUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    if (this.#shuttingDown) throw shuttingDownError();
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    return this.#verifyAuthoritatively(model, signal);
  }

  async acquireUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelUseGrant> {
    if (this.#shuttingDown) throw shuttingDownError();
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    let lease = await this.#access.acquireUse(modelId, signal);
    try {
      let status = await this.#metadataStatus(model);
      const marker = await this.#repository.readCompletionMarker(model);
      if (
        !marker.present &&
        status.state === 'missing' &&
        status.downloadedBytes === model.totalBytes
      ) {
        lease.release();
        const verified = await this.#verifyAuthoritatively(model, signal);
        lease = await this.#access.acquireUse(modelId, signal);
        status = await this.#metadataStatus(model);
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
      settled: Promise.resolve(makeStatus(this.#getModel(modelId), 'missing', 0, null, false)),
    };
    const onExternalAbort = () => {
      active.intent = 'external';
      controller.abort(signal?.reason);
    };
    if (signal?.aborted === true) onExternalAbort();
    else signal?.addEventListener('abort', onExternalAbort, { once: true });
    const settled = this.#withModelMutation(
      modelId,
      () => this.#download(modelId, active),
      active.controller.signal,
    )
      .catch((error: unknown) => {
        if (
          active.controller.signal.aborted &&
          error instanceof ModelManagerError &&
          error.code === 'CANCELLED'
        ) {
          return this.#finishCancelledDownload(this.#getModel(modelId), active);
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
    await this.#withModelMutation(modelId, async () => {
      await this.#repository.removeTemporaryRevision(this.#getModel(modelId));
      this.#states.delete(modelId);
    });
    return this.status(modelId);
  }

  retry(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    return this.download(modelId, signal);
  }

  async delete(modelId: WhisperModelId): Promise<ModelStatus> {
    await this.#abortActive(modelId, 'cancelled');
    await this.#withModelMutation(modelId, () => this.#deleteFiles(modelId));
    const status = await this.status(modelId);
    this.#emit(this.#getModel(modelId), status.state, null, status.downloadedBytes);
    return status;
  }

  async deleteIfIdle(modelId: WhisperModelId): Promise<ModelDeleteResult> {
    const model = this.#getModel(modelId);
    const lease = this.#access.tryAcquireMutation(modelId);
    if (lease === null) {
      return { outcome: 'in-use', status: await this.status(modelId) };
    }
    try {
      await this.#recoverAndRemember(model);
      await this.#beforeMutation?.(modelId);
      await this.#deleteFiles(modelId);
    } finally {
      try {
        await this.#recoverAndRemember(model);
      } finally {
        lease.release();
      }
    }
    const status = await this.status(modelId);
    this.#emit(model, status.state, null, status.downloadedBytes);
    return { outcome: 'deleted', status };
  }

  async shutdown(): Promise<void> {
    this.#shuttingDown = true;
    const active = this.#active;
    if (active !== null) {
      active.intent = 'shutdown';
      active.controller.abort('shutdown');
    }
    const verificationLifecycle = [...this.#verificationLifecycle];
    for (const verification of verificationLifecycle) verification.controller.abort('shutdown');
    await Promise.allSettled([
      ...(active === null ? [] : [active.settled]),
      ...verificationLifecycle.map((verification) => verification.promise),
    ]);
  }

  async #download(modelId: WhisperModelId, active: ActiveDownload): Promise<ModelStatus> {
    const model = this.#getModel(modelId);
    this.#states.delete(modelId);
    try {
      await this.#repository.prepareRoots();
      const installed = await this.#repository.inspectInstalled(
        model,
        active.controller.signal,
        true,
      );
      if (installed.valid) {
        active.state = 'verifying';
        this.#emit(model, 'verifying', null, model.totalBytes);
        await this.#afterInstallValidation?.(model.id, active.controller.signal);
        active.controller.signal.throwIfAborted();
        await this.#repository.commitVerification(model, installed.identities);
        this.#emit(model, 'ready', null, model.totalBytes);
        return makeStatus(model, 'ready', model.totalBytes, null, false);
      }

      await this.#repository.prepareStaging(model);
      const stagedHardLinksInstalledFiles = await this.#repository.reuseInstalledFiles(
        model,
        installed.identities,
        active.controller.signal,
      );
      const staged = await this.#repository.inspectStaging(model, active.controller.signal, true);
      let verified = staged;
      const completeStagingStillCurrent =
        staged.valid &&
        (await this.#repository.stagedIdentityStillCurrent(model, staged.identities));
      if (!completeStagingStillCurrent) {
        await this.#repository.ensureDownloadCapacity(model, staged.identities);
        let completed = 0;
        for (const file of model.files) {
          active.controller.signal.throwIfAborted();
          if (
            (await this.#repository.inspectStagedFile(model, file, active.controller.signal, true))
              .valid
          ) {
            await this.#repository.removePartial(model, file);
            completed += file.size;
            this.#emit(model, 'downloading', file, completed, file.size);
            continue;
          }
          await this.#repository.removeStagedFile(model, file);
          completed = await this.#downloadFile(model, file, completed, active);
        }
        active.state = 'verifying';
        this.#emit(model, 'verifying', null, model.totalBytes);
        verified = await this.#repository.inspectStaging(model, active.controller.signal, true);
        if (!verified.valid) {
          throw new ModelManagerError(
            'CORRUPT',
            'Downloaded staged model failed verification.',
            true,
          );
        }
      } else {
        active.state = 'verifying';
        this.#emit(model, 'verifying', null, model.totalBytes);
      }
      await this.#repository.prepareStagedPublication(model, verified.identities);
      active.controller.signal.throwIfAborted();
      active.state = 'installing';
      this.#emit(model, 'installing', null, model.totalBytes);
      await this.#repository.publishStagedRevision(model);
      try {
        await this.#repository.assertPublishedManifestEntries(model);
      } catch (error: unknown) {
        await this.#repository.removeInstalledRevision(model);
        throw error;
      }
      let installedIdentities = verified.identities;
      if (stagedHardLinksInstalledFiles) {
        const published = await this.#repository.inspectInstalled(
          model,
          active.controller.signal,
          true,
        );
        if (!published.valid) {
          throw new ModelManagerError(
            'CORRUPT',
            'Published model failed post-repair verification.',
            true,
          );
        }
        installedIdentities = published.identities;
      } else if (
        !(await this.#repository.installedIdentityStillCurrent(model, installedIdentities))
      ) {
        throw new ModelManagerError(
          'CORRUPT',
          'Published model identity changed during installation.',
          true,
        );
      }
      await this.#afterInstallValidation?.(model.id, active.controller.signal);
      active.controller.signal.throwIfAborted();
      await this.#repository.commitVerification(model, installedIdentities);
      this.#states.delete(modelId);
      this.#emit(model, 'ready', null, model.totalBytes);
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    } catch (error: unknown) {
      if (active.controller.signal.aborted) {
        return this.#finishCancelledDownload(model, active);
      }
      const mapped = mapDownloadError(error, active.state);
      if (mapped.code === 'WORKER_VALIDATION') {
        await this.#repository.removeCompletionMarker(model).catch(() => undefined);
      }
      const state: ModelState =
        mapped.code === 'OFFLINE' ? 'offline' : mapped.code === 'CORRUPT' ? 'corrupt' : 'error';
      this.#states.set(modelId, {
        state,
        detail: mapped.message,
        repairable: mapped.repairable,
      });
      const status = await this.#statusFromDisk(
        model,
        state,
        mapped.message,
        mapped.repairable,
        false,
      );
      this.#emit(model, state, null, status.downloadedBytes);
      throw mapped;
    }
  }

  async #finishCancelledDownload(
    model: ModelManifestEntry,
    active: ActiveDownload,
  ): Promise<ModelStatus> {
    if (active.intent === 'external') {
      throw new ModelManagerError('CANCELLED', 'Model download was cancelled.');
    }
    const state: ModelState = active.intent === 'paused' ? 'paused' : 'missing';
    const detail = active.intent === 'paused' ? 'Download paused.' : 'Download cancelled.';
    this.#states.set(model.id, { state, detail, repairable: false });
    const status = await this.#statusFromDisk(model, state, detail, false, false);
    this.#emit(model, state, null, status.downloadedBytes);
    return status;
  }

  async #downloadFile(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    completedBeforeFile: number,
    active: ActiveDownload,
  ): Promise<number> {
    await this.#repository.preparePartial(model, file);
    let offset = await this.#repository.partialSize(model, file);
    let verifiedPartIdentity: Omit<VerifiedModelFileIdentity, 'path'> | null = null;
    if (offset > file.size) {
      await this.#repository.removePartial(model, file);
      offset = 0;
    }
    if (offset === file.size) {
      const inspection = await this.#repository.inspectPartial(
        model,
        file,
        active.controller.signal,
      );
      if (inspection.valid && inspection.identity !== null) {
        verifiedPartIdentity = inspection.identity;
      } else {
        await this.#repository.removePartial(model, file);
        offset = 0;
      }
    }
    if (offset < file.size) {
      const response = await this.#transport.request(model, file, offset, active.controller.signal);
      let completedByConcurrentWriter = false;
      if (offset > 0 && validUnsatisfiedRange(response, file.size)) {
        await response.cancel();
        const currentSize = await this.#repository.partialSize(model, file);
        const inspection =
          currentSize === file.size
            ? await this.#repository.inspectPartial(model, file, active.controller.signal)
            : null;
        if (inspection?.valid === true && inspection.identity !== null) {
          offset = file.size;
          verifiedPartIdentity = inspection.identity;
          completedByConcurrentWriter = true;
        } else {
          throw new ModelManagerError(
            'PROTOCOL',
            'Range was unsatisfied before the file completed.',
          );
        }
      } else if (offset > 0 && response.status === 200) {
        try {
          await this.#repository.removePartial(model, file);
          offset = 0;
        } catch (error: unknown) {
          await response.cancel();
          throw error;
        }
      } else if (offset > 0 && !validContentRange(response, offset, file.size)) {
        await response.cancel();
        throw new ModelManagerError('PROTOCOL', 'Download server returned an invalid byte range.');
      } else if (offset === 0 && response.status !== 200) {
        await response.cancel();
        throw new ModelManagerError(
          'HTTP',
          `Model download failed with HTTP ${String(response.status)}.`,
        );
      }
      let written = offset;
      if (!completedByConcurrentWriter) {
        if (!response.hasBody) {
          throw new ModelManagerError('PROTOCOL', 'Download response had no body.');
        }
        let writer;
        try {
          writer = await this.#repository.openPartialWriter(model, file, offset);
        } catch (error: unknown) {
          await response.cancel();
          throw error;
        }
        let bodyComplete = false;
        try {
          for (;;) {
            active.controller.signal.throwIfAborted();
            const next = await response.read();
            if (next.done) {
              bodyComplete = true;
              break;
            }
            written += next.value.byteLength;
            if (written > file.size) {
              throw new ModelManagerError('PROTOCOL', 'Download exceeded its manifest size.');
            }
            await writer.write(next.value);
            this.#emit(model, 'downloading', file, completedBeforeFile + written, written);
          }
          await writer.sync();
        } finally {
          if (!bodyComplete) await response.cancel();
          await writer.close();
        }
        if (written !== file.size) {
          throw new ModelManagerError('PROTOCOL', 'Download ended before the declared size.');
        }
      }
    }
    await this.#repository.publishVerifiedPartial(
      model,
      file,
      verifiedPartIdentity,
      active.controller.signal,
    );
    this.#emit(model, 'downloading', file, completedBeforeFile + file.size, file.size);
    return completedBeforeFile + file.size;
  }

  #verifyAuthoritatively(model: ModelManifestEntry, signal?: AbortSignal): Promise<ModelStatus> {
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
    const taskLease = await this.#access.acquireUse(model.id, signal);
    try {
      const inspection = await this.#repository.inspectInstalled(model, signal, true);
      signal.throwIfAborted();
      if (inspection.valid) {
        try {
          await this.#afterInstallValidation?.(model.id, signal);
          signal.throwIfAborted();
          await this.#repository.commitVerification(model, inspection.identities);
        } catch (error: unknown) {
          const mapped = mapDownloadError(error, 'verifying');
          if (mapped.code === 'CANCELLED') throw mapped;
          await this.#repository.removeCompletionMarker(model).catch(() => undefined);
          this.#states.set(model.id, {
            state: 'error',
            detail: mapped.message,
            repairable: mapped.repairable,
          });
          return makeStatus(model, 'error', model.totalBytes, mapped.message, mapped.repairable);
        }
        this.#states.delete(model.id);
        return makeStatus(model, 'ready', model.totalBytes, null, false);
      }
      await this.#repository.removeCompletionMarker(model);
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

  async #metadataStatus(model: ModelManifestEntry): Promise<ModelStatus> {
    const override = this.#states.get(model.id);
    if (override !== undefined) {
      const inspection = await this.#repository.inspectInstalled(model, undefined, false);
      return makeStatus(
        model,
        override.state,
        Math.min(
          inspection.validBytes + (await this.#repository.temporaryBytes(model)),
          model.totalBytes,
        ),
        override.detail,
        override.repairable,
      );
    }
    const marker = await this.#repository.readCompletionMarker(model);
    if (
      marker.identity !== null &&
      (await this.#repository.installedIdentityStillCurrent(model, marker.identity))
    ) {
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    }
    const inspection = await this.#repository.inspectInstalled(model, undefined, false);
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
        inspection.validBytes + (await this.#repository.temporaryBytes(model)),
        model.totalBytes,
      ),
      inspection.corrupt ? 'Managed model files have invalid metadata.' : null,
      inspection.corrupt,
    );
  }

  async #statusFromDisk(
    model: ModelManifestEntry,
    state: ModelState,
    detail: string | null,
    repairable: boolean,
    hash: boolean,
  ): Promise<ModelStatus> {
    const inspection = await this.#repository.inspectInstalled(model, undefined, hash);
    return makeStatus(
      model,
      state,
      Math.min(
        inspection.validBytes + (await this.#repository.temporaryBytes(model)),
        model.totalBytes,
      ),
      detail,
      repairable,
    );
  }

  async #deleteFiles(modelId: WhisperModelId): Promise<void> {
    await this.#repository.deleteArtifacts(this.#getModel(modelId));
    this.#states.delete(modelId);
  }

  async #abortActive(modelId: WhisperModelId, intent: DownloadIntent): Promise<void> {
    const active = this.#active;
    if (active?.modelId !== modelId) return;
    active.intent = intent;
    active.controller.abort(intent);
    await active.settled.catch(() => undefined);
  }

  async #withModelMutation<Value>(
    modelId: WhisperModelId,
    operation: () => Promise<Value>,
    signal?: AbortSignal,
  ): Promise<Value> {
    const model = this.#getModel(modelId);
    await waitForModelTask(this.#ensureRecovered(model), signal, 'Model mutation was cancelled.');
    return this.#access.withMutation(
      modelId,
      async () => {
        await this.#recoverAndRemember(model);
        await this.#beforeMutation?.(modelId);
        try {
          return await operation();
        } finally {
          await this.#recoverAndRemember(model);
        }
      },
      signal,
    );
  }

  #ensureRecovered(model: ModelManifestEntry): Promise<void> {
    const existing = this.#recoveryTasks.get(model.id);
    if (existing !== undefined) return existing;
    const recovery = this.#access.withMutation(model.id, () =>
      this.#repository.recoverArtifacts(model),
    );
    this.#recoveryTasks.set(model.id, recovery);
    void recovery.catch(() => {
      if (this.#recoveryTasks.get(model.id) === recovery) this.#recoveryTasks.delete(model.id);
    });
    return recovery;
  }

  async #recoverAndRemember(model: ModelManifestEntry): Promise<void> {
    try {
      await this.#repository.recoverArtifacts(model);
      this.#recoveryTasks.set(model.id, Promise.resolve());
    } catch (error: unknown) {
      this.#recoveryTasks.delete(model.id);
      throw error;
    }
  }

  #getModel(modelId: WhisperModelId): ModelManifestEntry {
    const model = this.#manifest.models.find((candidate) => candidate.id === modelId);
    if (model === undefined) throw new Error(`Unsupported Whisper model: ${modelId}`);
    return model;
  }

  #emit(
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

function makeStatus(
  model: ModelManifestEntry,
  state: ModelState,
  downloadedBytes: number,
  detail: string | null,
  repairable: boolean,
): ModelStatus {
  return ModelStatusSchema.parse({
    modelId: model.id,
    state,
    downloadedBytes,
    totalBytes: model.totalBytes,
    detail,
    repairable,
  });
}

function shuttingDownError(): ModelManagerError {
  return new ModelManagerError('CANCELLED', 'Model manager is shutting down.');
}

function waitForModelTask<Value>(
  operation: Promise<Value>,
  signal: AbortSignal | undefined,
  cancellationMessage: string,
): Promise<Value> {
  if (signal === undefined) return operation;
  if (signal.aborted) {
    return Promise.reject(new ModelManagerError('CANCELLED', cancellationMessage));
  }
  return new Promise<Value>((resolveTask, reject) => {
    let settled = false;
    const finish = (callback: () => void): void => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      callback();
    };
    const abort = (): void =>
      finish(() => reject(new ModelManagerError('CANCELLED', cancellationMessage)));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (value) => finish(() => resolveTask(value)),
      (error: unknown) =>
        finish(() =>
          reject(
            error instanceof Error
              ? error
              : new ModelManagerError('IO', 'Model operation failed.', true),
          ),
        ),
    );
  });
}

function validUnsatisfiedRange(response: ModelDownloadResponse, total: number): boolean {
  if (response.status !== 416) return false;
  const match = /^bytes \*\/(\d+)$/.exec(response.header('content-range') ?? '');
  return match !== null && Number(match[1]) === total;
}

function validContentRange(
  response: ModelDownloadResponse,
  offset: number,
  total: number,
): boolean {
  if (response.status !== 206) return false;
  const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.header('content-range') ?? '');
  return (
    match !== null &&
    Number(match[1]) === offset &&
    Number(match[3]) === total &&
    Number(match[2]) === total - 1
  );
}

function mapDownloadError(
  error: unknown,
  phase: Extract<ModelState, 'downloading' | 'verifying' | 'installing'>,
): ModelManagerError {
  if (error instanceof ModelManagerError) return error;
  if (error instanceof WhisperClientError && error.code === 'CANCELLED') {
    return new ModelManagerError('CANCELLED', 'Model verification was cancelled.');
  }
  if (error instanceof WhisperClientError) {
    return new ModelManagerError(
      'WORKER_VALIDATION',
      error.code === 'MODEL_CORRUPT'
        ? 'The installed model passed download verification but the offline Whisper worker rejected it as corrupt. Retry will reuse files that still pass SHA-256 verification.'
        : 'The installed model passed SHA-256 verification but the offline Whisper worker could not validate it. Restart Talking Quill, then retry without redownloading verified files.',
      true,
    );
  }
  if (error instanceof DOMException && error.name === 'AbortError') {
    return new ModelManagerError('CANCELLED', 'Model verification was cancelled.');
  }
  if (error instanceof DOMException && error.name === 'TimeoutError') {
    return new ModelManagerError('TIMEOUT', 'Model download request timed out.', true);
  }
  const code =
    typeof error === 'object' && error !== null && 'code' in error ? String(error.code) : '';
  if (['ENOSPC', 'EDQUOT'].includes(code)) {
    return new ModelManagerError(
      'DISK_SPACE',
      'The model could not be saved because the model drive is out of available space.',
      true,
    );
  }
  if (isTransientWindowsFileError(error)) {
    return new ModelManagerError(
      'FILE_LOCKED',
      phase === 'installing'
        ? 'Windows could not install the verified model because a file is temporarily locked. Retry to reuse the verified download.'
        : 'Windows could not update a model file because it is locked or access was denied. Close security scans or other Talking Quill instances, then retry.',
      true,
    );
  }
  if (['ENETUNREACH', 'ENOTFOUND', 'ECONNREFUSED', 'EAI_AGAIN'].includes(code)) {
    return new ModelManagerError(
      'OFFLINE',
      'The model host is unreachable. A completed cached model remains available offline.',
    );
  }
  return new ModelManagerError(
    'IO',
    phase === 'verifying'
      ? 'The downloaded model could not be verified. Retry will reuse files that still pass verification.'
      : phase === 'installing'
        ? 'The verified model could not be installed. Retry will reuse the completed download.'
        : 'The model file could not be read or written. Retry will resume from safe existing bytes.',
    true,
  );
}

function isTransientWindowsFileError(error: unknown): boolean {
  return (
    process.platform === 'win32' &&
    ['EBUSY', 'EPERM', 'EACCES'].some((code) => hasCode(error, code))
  );
}

function hasCode(error: unknown, code: string): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error as { readonly code?: unknown }).code === code
  );
}
