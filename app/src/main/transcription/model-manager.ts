import { randomUUID } from 'node:crypto';
import { constants, type Stats } from 'node:fs';
import {
  lstat,
  mkdir,
  open,
  readdir,
  rename,
  rm,
  statfs,
  writeFile,
  type FileHandle,
} from 'node:fs/promises';
import { basename, dirname, join, relative, resolve } from 'node:path';
import {
  MODEL_DOWNLOAD_HEADROOM_RATIO,
  MODEL_DOWNLOAD_MAX_REDIRECTS,
  MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
  MODEL_DOWNLOAD_REQUEST_TIMEOUT_MS,
} from '../../shared/constants/whisper';
import type {
  ModelManifest,
  ModelManifestEntry,
  ModelManifestFile,
  WhisperModelId,
  VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import { VerifiedModelFileIdentitySchema } from '../../shared/schemas/model-manifest';
import {
  ModelProgressSchema,
  ModelStatusSchema,
  type ModelDeleteResult,
  type ModelProgress,
  type ModelState,
  type ModelStatus,
} from '../../shared/schemas/transcription';
import type { EgressObserver } from '../security/egress-audit';
import { ModelManagerError, WhisperClientError } from './errors';
import { inspectFile, sameVerifiedIdentity } from './model-integrity';
import { ModelAccessCoordinator } from './model-access-coordinator';
import { MODEL_MANIFEST } from './model-manifest';

const COMPLETION_MARKER = '.talking-quill-complete.json';
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
  /** Test seam for platform rename behavior; production always uses node:fs/promises. */
  readonly rename?: typeof rename;
}

export class ModelManager {
  readonly #modelsDirectory: string;
  readonly #temporaryDirectory: string;
  readonly #fetch: typeof fetch;
  readonly #availableBytes: (path: string) => Promise<number>;
  readonly #urlFor: (model: ModelManifestEntry, file: ModelManifestFile) => string;
  readonly #validateRequestUrl: (url: string) => boolean;
  readonly #requestTimeoutMs: number;
  readonly #manifest: ModelManifest;
  readonly #access: ModelAccessCoordinator;
  readonly #inspectFile: typeof inspectFile;
  readonly #observeEgress: EgressObserver;
  readonly #rename: typeof rename;
  readonly #listeners = new Set<(event: ModelProgress) => void>();
  readonly #states = new Map<WhisperModelId, StateOverride>();
  readonly #verificationTasks = new Map<WhisperModelId, SharedVerification>();
  readonly #recoveryTasks = new Map<WhisperModelId, Promise<void>>();
  #beforeMutation: ((modelId: WhisperModelId) => Promise<void>) | null = null;
  #afterInstallValidation:
    ((modelId: WhisperModelId, signal: AbortSignal) => Promise<void>) | null = null;
  #active: ActiveDownload | null = null;
  #shuttingDown = false;

  constructor(options: ModelManagerOptions) {
    this.#modelsDirectory = resolve(options.modelsDirectory);
    this.#temporaryDirectory = resolve(options.temporaryDirectory);
    this.#fetch = options.fetch ?? fetch;
    this.#availableBytes = options.availableBytes ?? defaultAvailableBytes;
    this.#urlFor = options.urlFor ?? defaultModelUrl;
    this.#validateRequestUrl = options.validateRequestUrl ?? defaultValidateRequestUrl;
    this.#requestTimeoutMs = options.requestTimeoutMs ?? MODEL_DOWNLOAD_REQUEST_TIMEOUT_MS;
    this.#manifest = options.manifest ?? MODEL_MANIFEST;
    this.#access = options.accessCoordinator ?? new ModelAccessCoordinator();
    this.#inspectFile = options.inspectFile ?? inspectFile;
    this.#observeEgress = options.observeEgress ?? (() => undefined);
    this.#rename = options.rename ?? rename;
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
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    if (this.#active?.modelId === modelId) {
      return this.#statusFromDisk(model, this.#active.state, null, false, false);
    }
    if (!verify) return this.#metadataStatus(model);
    return this.#verifyAuthoritatively(model);
  }

  async verifyForUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    return this.#verifyAuthoritatively(model, signal);
  }

  async acquireUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelUseGrant> {
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    let lease = await this.#access.acquireUse(modelId, signal);
    try {
      let status = await this.#metadataStatus(model);
      const marker = await this.#readMarker(model);
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
        ? this.#active.settled
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
    const settled = this.#withModelMutation(modelId, () => this.#download(modelId, active)).finally(
      () => {
        signal?.removeEventListener('abort', onExternalAbort);
        if (this.#active === active) this.#active = null;
      },
    );
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
      await rm(this.#temporaryModelDirectory(this.#getModel(modelId)), {
        recursive: true,
        force: true,
      });
      this.#states.delete(modelId);
    });
    return this.status(modelId);
  }

  retry(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelStatus> {
    this.#states.delete(modelId);
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
      await this.#recoverModelArtifacts(model);
      await this.#beforeMutation?.(modelId);
      await this.#deleteFiles(modelId);
    } finally {
      try {
        await this.#recoverModelArtifacts(model);
        this.#recoveryTasks.set(modelId, Promise.resolve());
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
      await active.settled.catch(() => undefined);
    }
  }

  async #download(modelId: WhisperModelId, active: ActiveDownload): Promise<ModelStatus> {
    const model = this.#getModel(modelId);
    this.#states.delete(modelId);
    try {
      await ensureSafeDirectory(this.#modelsDirectory, this.#modelsDirectory);
      await ensureSafeDirectory(this.#temporaryDirectory, this.#temporaryDirectory);
      const installed = await this.#inspectModel(model, active.controller.signal, true);
      if (installed.valid) {
        active.state = 'verifying';
        this.#emit(model, 'verifying', null, model.totalBytes);
        await this.#afterInstallValidation?.(model.id, active.controller.signal);
        active.controller.signal.throwIfAborted();
        await this.#commitVerification(model, installed.identities);
        this.#emit(model, 'ready', null, model.totalBytes);
        return makeStatus(model, 'ready', model.totalBytes, null, false);
      }

      const stagedDirectory = this.#temporaryModelDirectory(model);
      await ensureSafeDirectory(this.#temporaryDirectory, stagedDirectory);
      const staged = await this.#inspectStagedModel(model, active.controller.signal, true);
      let verified = staged;
      const completeStagingStillCurrent =
        staged.valid &&
        (await this.#identityStillCurrent(
          model,
          staged.identities,
          stagedDirectory,
          this.#temporaryDirectory,
        ));
      if (!completeStagingStillCurrent) {
        const partialBytes = await this.#partialBytes(model);
        const remaining = Math.max(0, model.totalBytes - staged.validBytes - partialBytes);
        const headroom = Math.max(
          MODEL_DOWNLOAD_MINIMUM_HEADROOM_BYTES,
          Math.ceil(remaining * MODEL_DOWNLOAD_HEADROOM_RATIO),
        );
        if (
          remaining > 0 &&
          (await this.#availableBytes(this.#modelsDirectory)) < remaining + headroom
        ) {
          throw new ModelManagerError(
            'DISK_SPACE',
            'Not enough disk space to download this model.',
          );
        }

        let completed = 0;
        for (const file of model.files) {
          active.controller.signal.throwIfAborted();
          const stagedTarget = this.#stagedTargetPath(model, file);
          if (
            (
              await this.#inspectFile(
                stagedTarget,
                file.size,
                file.sha256,
                true,
                active.controller.signal,
              )
            ).valid
          ) {
            await rm(this.#partPath(model, file), { force: true });
            completed += file.size;
            this.#emit(model, 'downloading', file, completed, file.size);
            continue;
          }
          await rm(stagedTarget, { force: true });
          completed = await this.#downloadFile(model, file, completed, active);
        }
        active.state = 'verifying';
        this.#emit(model, 'verifying', null, model.totalBytes);
        verified = await this.#inspectStagedModel(model, active.controller.signal, true);
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
      await this.#prepareStagedPublication(model, verified.identities);
      active.controller.signal.throwIfAborted();
      active.state = 'installing';
      this.#emit(model, 'installing', null, model.totalBytes);
      const installedDirectory = this.#modelDirectory(model);
      await publishRevisionDirectory(stagedDirectory, installedDirectory, this.#rename);
      try {
        await this.#assertOnlyManifestEntries(model, installedDirectory);
      } catch (error: unknown) {
        await rm(installedDirectory, { recursive: true, force: true });
        throw error;
      }
      if (!(await this.#identityStillCurrent(model, verified.identities))) {
        throw new ModelManagerError(
          'CORRUPT',
          'Published model identity changed during installation.',
          true,
        );
      }
      await this.#afterInstallValidation?.(model.id, active.controller.signal);
      active.controller.signal.throwIfAborted();
      await this.#commitVerification(model, verified.identities);
      this.#states.delete(modelId);
      this.#emit(model, 'ready', null, model.totalBytes);
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    } catch (error: unknown) {
      if (active.controller.signal.aborted) {
        if (active.intent === 'external') {
          throw new ModelManagerError('CANCELLED', 'Model download was cancelled.');
        }
        const state: ModelState = active.intent === 'paused' ? 'paused' : 'missing';
        const detail = active.intent === 'paused' ? 'Download paused.' : 'Download cancelled.';
        this.#states.set(modelId, { state, detail, repairable: false });
        const status = await this.#statusFromDisk(model, state, detail, false, false);
        this.#emit(model, state, null, status.downloadedBytes);
        return status;
      }
      const mapped = mapDownloadError(error, active.state);
      if (mapped.code === 'WORKER_VALIDATION') {
        await rm(join(this.#modelDirectory(model), COMPLETION_MARKER), { force: true }).catch(
          () => undefined,
        );
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

  async #downloadFile(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    completedBeforeFile: number,
    active: ActiveDownload,
  ): Promise<number> {
    const part = this.#partPath(model, file);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(part));
    let offset = await safeRegularFileSize(part);
    let verifiedPartIdentity: Omit<VerifiedModelFileIdentity, 'path'> | null = null;
    if (offset > file.size) {
      await rm(part, { force: true });
      offset = 0;
    }
    if (offset === file.size) {
      const inspection = await this.#inspectFile(
        part,
        file.size,
        file.sha256,
        true,
        active.controller.signal,
      );
      if (inspection.valid && inspection.identity !== null) {
        verifiedPartIdentity = inspection.identity;
      } else {
        await rm(part, { force: true });
        offset = 0;
      }
    }
    if (offset < file.size) {
      const headers = {
        'Accept-Encoding': 'identity',
        ...(offset > 0 ? { Range: `bytes=${String(offset)}-` } : {}),
      };
      const response = await this.#fetchWithRedirects(this.#urlFor(model, file), headers, active);
      let completedByConcurrentWriter = false;
      if (offset > 0 && response.status === 416 && validUnsatisfiedRange(response, file.size)) {
        await cancelResponseBody(response);
        const currentSize = await safeRegularFileSize(part);
        const inspection =
          currentSize === file.size
            ? await this.#inspectFile(part, file.size, file.sha256, true, active.controller.signal)
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
        await rm(part, { force: true });
        offset = 0;
      } else if (offset > 0 && !validContentRange(response, offset, file.size)) {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Download server returned an invalid byte range.');
      } else if (offset === 0 && response.status !== 200) {
        await cancelResponseBody(response);
        throw new ModelManagerError(
          'HTTP',
          `Model download failed with HTTP ${String(response.status)}.`,
        );
      }
      let written = offset;
      if (!completedByConcurrentWriter) {
        if (response.body === null) {
          throw new ModelManagerError('PROTOCOL', 'Download response had no body.');
        }
        const handle = await openSafePart(part, offset);
        const reader = response.body.getReader();
        let bodyComplete = false;
        try {
          for (;;) {
            active.controller.signal.throwIfAborted();
            const next = await readWithInactivityTimeout(
              reader,
              this.#requestTimeoutMs,
              active.controller.signal,
            );
            if (next.done) {
              bodyComplete = true;
              break;
            }
            written += next.value.byteLength;
            if (written > file.size) {
              throw new ModelManagerError('PROTOCOL', 'Download exceeded its manifest size.');
            }
            await writeAll(handle, next.value);
            this.#emit(model, 'downloading', file, completedBeforeFile + written, written);
          }
          await handle.sync();
        } finally {
          if (!bodyComplete) await reader.cancel().catch(() => undefined);
          await handle.close();
        }
        if (written !== file.size) {
          throw new ModelManagerError('PROTOCOL', 'Download ended before the declared size.');
        }
      }
    }
    if (verifiedPartIdentity === null) {
      const inspection = await this.#inspectFile(
        part,
        file.size,
        file.sha256,
        true,
        active.controller.signal,
      );
      if (!inspection.valid || inspection.identity === null) {
        await rm(part, { force: true });
        throw new ModelManagerError('CORRUPT', `Checksum failed for ${file.path}.`, true);
      }
      verifiedPartIdentity = inspection.identity;
    }
    if (!(await verifiedIdentityStillCurrent(part, verifiedPartIdentity))) {
      await rm(part, { force: true });
      throw new ModelManagerError('CORRUPT', `Verified file changed for ${file.path}.`, true);
    }
    const target = this.#stagedTargetPath(model, file);
    await ensureSafeDirectory(this.#temporaryDirectory, dirname(target));
    await publishStagedFile(part, target, this.#rename);
    this.#emit(model, 'downloading', file, completedBeforeFile + file.size, file.size);
    return completedBeforeFile + file.size;
  }

  async #fetchWithRedirects(
    initialUrl: string,
    headers: Readonly<Record<string, string>> | undefined,
    active: ActiveDownload,
  ): Promise<Response> {
    let current = new URL(initialUrl);
    for (let redirectCount = 0; redirectCount <= MODEL_DOWNLOAD_MAX_REDIRECTS; redirectCount += 1) {
      if (!this.#validateRequestUrl(current.href)) {
        throw new ModelManagerError('PROTOCOL', 'Model download destination is not trusted.');
      }
      const timeoutController = new AbortController();
      const timeout = setTimeout(
        () => timeoutController.abort('request timeout'),
        this.#requestTimeoutMs,
      );
      timeout.unref();
      const signal = AbortSignal.any([active.controller.signal, timeoutController.signal]);
      let response: Response;
      try {
        this.#observeEgress('model-download');
        response = await this.#fetch(current, {
          ...(headers === undefined ? {} : { headers }),
          redirect: 'manual',
          signal,
        });
      } catch (error: unknown) {
        if (timeoutController.signal.aborted && !active.controller.signal.aborted) {
          throw new ModelManagerError('TIMEOUT', 'Model download request timed out.', true);
        }
        throw error;
      } finally {
        clearTimeout(timeout);
      }
      if (![301, 302, 303, 307, 308].includes(response.status)) return response;
      const location = response.headers.get('location');
      if (location === null) {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Model download redirect had no destination.');
      }
      if (redirectCount === MODEL_DOWNLOAD_MAX_REDIRECTS) {
        await cancelResponseBody(response);
        throw new ModelManagerError('PROTOCOL', 'Model download exceeded the redirect limit.');
      }
      const next = new URL(location, current);
      await cancelResponseBody(response);
      current = next;
    }
    throw new ModelManagerError('PROTOCOL', 'Model download redirect failed.');
  }

  #inspectModel(model: ModelManifestEntry, signal?: AbortSignal, hash = true) {
    return this.#inspectModelDirectory(
      model,
      this.#modelDirectory(model),
      this.#modelsDirectory,
      signal,
      hash,
    );
  }

  #inspectStagedModel(model: ModelManifestEntry, signal?: AbortSignal, hash = true) {
    return this.#inspectModelDirectory(
      model,
      this.#temporaryModelDirectory(model),
      this.#temporaryDirectory,
      signal,
      hash,
    );
  }

  async #inspectModelDirectory(
    model: ModelManifestEntry,
    directory: string,
    managedRoot: string,
    signal?: AbortSignal,
    hash = true,
  ) {
    let validBytes = 0;
    let existingBytes = 0;
    let corrupt = false;
    const identities: VerifiedModelFileIdentity[] = [];
    try {
      for (const file of model.files) {
        const target = join(directory, ...file.path.split('/'));
        await assertSafeExistingDirectoryChain(managedRoot, dirname(target));
      }
    } catch (error: unknown) {
      if (error instanceof ModelManagerError && error.code === 'CORRUPT') {
        return {
          valid: false,
          validBytes: 0,
          existingBytes: 1,
          corrupt: true,
          identities,
        } as const;
      }
      throw error;
    }
    for (const file of model.files) {
      signal?.throwIfAborted();
      const target = join(directory, ...file.path.split('/'));
      const result = await this.#inspectFile(target, file.size, file.sha256, hash, signal);
      if (result.exists) existingBytes += result.size;
      if (result.valid) {
        validBytes += file.size;
        if (result.identity !== null) identities.push({ path: file.path, ...result.identity });
      } else if (result.exists) corrupt = true;
    }
    return {
      valid: validBytes === model.totalBytes && identities.length === model.files.length,
      validBytes,
      existingBytes,
      corrupt,
      identities,
    } as const;
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
      const inspection = await this.#inspectModel(model, signal, true);
      signal.throwIfAborted();
      if (inspection.valid) {
        try {
          await this.#afterInstallValidation?.(model.id, signal);
          signal.throwIfAborted();
          await this.#commitVerification(model, inspection.identities);
        } catch (error: unknown) {
          const mapped = mapDownloadError(error, 'verifying');
          if (mapped.code === 'CANCELLED') throw mapped;
          await rm(join(this.#modelDirectory(model), COMPLETION_MARKER), { force: true }).catch(
            () => undefined,
          );
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
      await rm(join(this.#modelDirectory(model), COMPLETION_MARKER), { force: true });
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
              error instanceof Error
                ? error
                : new ModelManagerError('IO', 'Model verification failed.', true),
            ),
          ),
      );
    });
  }

  #finishSharedVerification(modelId: WhisperModelId, verification: SharedVerification): void {
    verification.settled = true;
    if (this.#verificationTasks.get(modelId) === verification) {
      this.#verificationTasks.delete(modelId);
    }
  }

  async #metadataStatus(model: ModelManifestEntry): Promise<ModelStatus> {
    const override = this.#states.get(model.id);
    if (override !== undefined) {
      const inspection = await this.#inspectModel(model, undefined, false);
      return makeStatus(
        model,
        override.state,
        Math.min(inspection.validBytes + (await this.#temporaryBytes(model)), model.totalBytes),
        override.detail,
        override.repairable,
      );
    }
    const marker = await this.#readMarker(model);
    if (marker.identity !== null && (await this.#identityStillCurrent(model, marker.identity))) {
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    }
    const inspection = await this.#inspectModel(model, undefined, false);
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
      Math.min(inspection.validBytes + (await this.#temporaryBytes(model)), model.totalBytes),
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
    const inspection = await this.#inspectModel(model, undefined, hash);
    return makeStatus(
      model,
      state,
      Math.min(inspection.validBytes + (await this.#temporaryBytes(model)), model.totalBytes),
      detail,
      repairable,
    );
  }

  async #temporaryBytes(model: ModelManifestEntry): Promise<number> {
    let total = 0;
    for (const file of model.files) {
      const staged = await safeRegularFileSize(this.#stagedTargetPath(model, file));
      if (staged === file.size) total += file.size;
      else total += Math.min(await safeRegularFileSize(this.#partPath(model, file)), file.size);
    }
    return total;
  }

  async #partialBytes(model: ModelManifestEntry): Promise<number> {
    let total = 0;
    for (const file of model.files) {
      total += Math.min(await safeRegularFileSize(this.#partPath(model, file)), file.size);
    }
    return total;
  }

  async #commitVerification(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    if (identities.length !== model.files.length) {
      throw new ModelManagerError('CORRUPT', 'Verified model identity was incomplete.', true);
    }
    await this.#writeMarkerAt(
      model,
      this.#modelDirectory(model),
      identities,
      this.#modelsDirectory,
    );
  }

  async #writeMarkerAt(
    model: ModelManifestEntry,
    directory: string,
    identities: readonly VerifiedModelFileIdentity[],
    managedRoot: string,
  ): Promise<void> {
    await ensureSafeDirectory(managedRoot, directory);
    const temporary = join(directory, `${COMPLETION_MARKER}.${randomUUID()}.tmp`);
    await writeFile(
      temporary,
      `${JSON.stringify({ schemaVersion: 1, revision: model.revision, totalBytes: model.totalBytes, files: identities })}\n`,
      { mode: 0o600, flag: 'wx' },
    );
    await publishAtomically(temporary, join(directory, COMPLETION_MARKER), this.#rename);
  }

  async #readMarker(model: ModelManifestEntry): Promise<{
    readonly present: boolean;
    readonly identity: readonly VerifiedModelFileIdentity[] | null;
  }> {
    const path = join(this.#modelDirectory(model), COMPLETION_MARKER);
    let text: string;
    try {
      const before = await lstat(path);
      if (!before.isFile() || before.isSymbolicLink() || before.size > 64 * 1024) {
        return { present: true, identity: null };
      }
      const handle = await open(path, constants.O_RDONLY | constants.O_NOFOLLOW);
      try {
        const opened = await handle.stat();
        if (!opened.isFile() || !sameOpenedFile(before, opened)) {
          return { present: true, identity: null };
        }
        text = await handle.readFile('utf8');
        const [afterHandle, afterPath] = await Promise.all([handle.stat(), lstat(path)]);
        if (
          afterPath.isSymbolicLink() ||
          !afterPath.isFile() ||
          !sameOpenedFile(opened, afterHandle) ||
          !sameOpenedFile(afterHandle, afterPath)
        ) {
          return { present: true, identity: null };
        }
      } finally {
        await handle.close();
      }
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) return { present: false, identity: null };
      return { present: true, identity: null };
    }
    try {
      const value: unknown = JSON.parse(text);
      if (typeof value !== 'object' || value === null) return { present: true, identity: null };
      const record = value as Readonly<Record<string, unknown>>;
      const files = VerifiedModelFileIdentitySchema.array()
        .length(model.files.length)
        .safeParse(record.files);
      if (
        record.schemaVersion !== 1 ||
        record.revision !== model.revision ||
        record.totalBytes !== model.totalBytes ||
        !files.success ||
        files.data.some((file, index) => file.path !== model.files[index]?.path)
      ) {
        return { present: true, identity: null };
      }
      return { present: true, identity: files.data };
    } catch {
      return { present: true, identity: null };
    }
  }

  async #identityStillCurrent(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
    directory = this.#modelDirectory(model),
    managedRoot = this.#modelsDirectory,
  ): Promise<boolean> {
    for (const expected of identities) {
      const file = model.files.find((candidate) => candidate.path === expected.path);
      if (file?.size !== expected.size) return false;
      const target = join(directory, ...file.path.split('/'));
      try {
        await assertSafeExistingDirectoryChain(managedRoot, dirname(target));
      } catch {
        return false;
      }
      try {
        const metadata = await lstat(target);
        if (
          !metadata.isFile() ||
          metadata.isSymbolicLink() ||
          !sameVerifiedIdentity(expected, metadata)
        ) {
          return false;
        }
      } catch {
        return false;
      }
    }
    return true;
  }

  async #prepareStagedPublication(
    model: ModelManifestEntry,
    identities: readonly VerifiedModelFileIdentity[],
  ): Promise<void> {
    await Promise.all(model.files.map((file) => rm(this.#partPath(model, file), { force: true })));
    await this.#assertOnlyManifestEntries(model);
    if (
      !(await this.#identityStillCurrent(
        model,
        identities,
        this.#temporaryModelDirectory(model),
        this.#temporaryDirectory,
      ))
    ) {
      throw new ModelManagerError('CORRUPT', 'Verified staging changed before publication.', true);
    }
  }

  async #assertOnlyManifestEntries(
    model: ModelManifestEntry,
    rootDirectory = this.#temporaryModelDirectory(model),
  ): Promise<void> {
    const expectedFiles = new Set(model.files.map((file) => file.path));
    const expectedDirectories = new Set<string>();
    for (const file of model.files) {
      const segments = file.path.split('/');
      for (let index = 1; index < segments.length; index += 1) {
        expectedDirectories.add(segments.slice(0, index).join('/'));
      }
    }

    const visit = async (directory: string, relativeDirectory: string): Promise<void> => {
      const entries = await readdir(directory, { withFileTypes: true });
      for (const entry of entries) {
        const relativePath =
          relativeDirectory === '' ? entry.name : `${relativeDirectory}/${entry.name}`;
        if (expectedFiles.has(relativePath)) {
          if (!entry.isFile() || entry.isSymbolicLink()) {
            throw new ModelManagerError(
              'CORRUPT',
              'Model staging contains an invalid manifest entry.',
              true,
            );
          }
          continue;
        }
        if (
          expectedDirectories.has(relativePath) &&
          entry.isDirectory() &&
          !entry.isSymbolicLink()
        ) {
          await visit(join(directory, entry.name), relativePath);
          continue;
        }
        throw new ModelManagerError(
          'CORRUPT',
          `Model staging contains unexpected entry: ${relativePath}.`,
          true,
        );
      }
    };

    await visit(rootDirectory, '');
  }

  async #deleteFiles(modelId: WhisperModelId): Promise<void> {
    const model = this.#getModel(modelId);
    await Promise.all([
      rm(this.#modelDirectory(model), { recursive: true, force: true }),
      rm(this.#temporaryModelDirectory(model), { recursive: true, force: true }),
    ]);
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
  ): Promise<Value> {
    const model = this.#getModel(modelId);
    await this.#ensureRecovered(model);
    return this.#access.withMutation(modelId, async () => {
      await this.#recoverModelArtifacts(model);
      await this.#beforeMutation?.(modelId);
      try {
        return await operation();
      } finally {
        await this.#recoverModelArtifacts(model);
        this.#recoveryTasks.set(modelId, Promise.resolve());
      }
    });
  }

  #ensureRecovered(model: ModelManifestEntry): Promise<void> {
    const existing = this.#recoveryTasks.get(model.id);
    if (existing !== undefined) return existing;
    const recovery = this.#access.withMutation(model.id, () => this.#recoverModelArtifacts(model));
    this.#recoveryTasks.set(model.id, recovery);
    return recovery;
  }

  async #recoverModelArtifacts(model: ModelManifestEntry): Promise<void> {
    const target = this.#modelDirectory(model);
    await ensureSafeDirectory(this.#modelsDirectory, dirname(target));
    await recoverRevisionDirectory(target);
    await ensureSafeDirectory(
      this.#temporaryDirectory,
      dirname(this.#temporaryModelDirectory(model)),
    );
  }

  #getModel(modelId: WhisperModelId): ModelManifestEntry {
    const model = this.#manifest.models.find((candidate) => candidate.id === modelId);
    if (model === undefined) throw new Error(`Unsupported Whisper model: ${modelId}`);
    return model;
  }

  #stagedTargetPath(model: ModelManifestEntry, file: ModelManifestFile): string {
    return join(this.#temporaryModelDirectory(model), ...file.path.split('/'));
  }

  #partPath(model: ModelManifestEntry, file: ModelManifestFile): string {
    return `${this.#stagedTargetPath(model, file)}.part`;
  }

  #modelDirectory(model: ModelManifestEntry): string {
    return join(this.#modelsDirectory, ...model.id.split('/'), model.revision);
  }

  #temporaryModelDirectory(model: ModelManifestEntry): string {
    return join(this.#temporaryDirectory, ...model.id.split('/'), model.revision);
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
    for (const listener of this.#listeners) listener(event);
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

async function ensureSafeDirectory(root: string, destination: string): Promise<void> {
  const absoluteRoot = resolve(root);
  const absoluteDestination = resolve(destination);
  const suffix = relative(absoluteRoot, absoluteDestination);
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  await mkdir(absoluteRoot, { recursive: true, mode: 0o700 });
  let current = absoluteRoot;
  await assertDirectoryNotLink(current);
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await mkdir(current, { mode: 0o700 });
    } catch (error: unknown) {
      if (!hasCode(error, 'EEXIST')) throw error;
    }
    await assertDirectoryNotLink(current);
  }
}

async function assertDirectoryNotLink(path: string): Promise<void> {
  const metadata = await lstat(path);
  if (!metadata.isDirectory() || metadata.isSymbolicLink()) {
    throw new ModelManagerError('CORRUPT', 'Managed model directory contains a link.', true);
  }
}

/** Validates existing parents without turning a read-only status check into a filesystem mutation. */
async function assertSafeExistingDirectoryChain(root: string, destination: string): Promise<void> {
  const absoluteRoot = resolve(root);
  const suffix = relative(absoluteRoot, resolve(destination));
  if (
    suffix.startsWith('..') ||
    suffix.includes(`..${process.platform === 'win32' ? '\\' : '/'}`)
  ) {
    throw new ModelManagerError('PROTOCOL', 'Model path escaped its managed directory.');
  }
  let current = absoluteRoot;
  for (const segment of suffix.split(/[\\/]/).filter(Boolean)) {
    current = join(current, segment);
    try {
      await assertDirectoryNotLink(current);
    } catch (error: unknown) {
      if (hasCode(error, 'ENOENT')) return;
      throw error;
    }
  }
}

async function safeRegularFileSize(path: string): Promise<number> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink()) {
      throw new ModelManagerError('CORRUPT', 'Managed model file is not a regular file.', true);
    }
    return metadata.size;
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return 0;
    throw error;
  }
}

async function verifiedIdentityStillCurrent(
  path: string,
  expected: Omit<VerifiedModelFileIdentity, 'path'>,
): Promise<boolean> {
  try {
    const metadata = await lstat(path);
    return (
      metadata.isFile() && !metadata.isSymbolicLink() && sameVerifiedIdentity(expected, metadata)
    );
  } catch {
    return false;
  }
}

async function openSafePart(path: string, offset: number): Promise<FileHandle> {
  const flags =
    constants.O_WRONLY |
    constants.O_CREAT |
    constants.O_NOFOLLOW |
    (offset === 0 ? constants.O_TRUNC : constants.O_APPEND);
  const handle = await open(path, flags, 0o600);
  const metadata = await handle.stat();
  let pathMetadata: Stats;
  try {
    pathMetadata = await lstat(path);
  } catch (error: unknown) {
    await handle.close();
    throw error;
  }
  if (
    !metadata.isFile() ||
    metadata.size !== offset ||
    pathMetadata.isSymbolicLink() ||
    !pathMetadata.isFile() ||
    !sameOpenedFile(metadata, pathMetadata)
  ) {
    await handle.close();
    throw new ModelManagerError(
      'CORRUPT',
      'Download staging path changed during secure open.',
      true,
    );
  }
  return handle;
}

async function writeAll(handle: FileHandle, bytes: Uint8Array): Promise<void> {
  let offset = 0;
  while (offset < bytes.byteLength) {
    const result = await handle.write(bytes, offset, bytes.byteLength - offset);
    if (result.bytesWritten <= 0) throw new ModelManagerError('IO', 'Unable to write model data.');
    offset += result.bytesWritten;
  }
}

function readWithInactivityTimeout(
  reader: ReadableStreamDefaultReader<Uint8Array>,
  timeoutMs: number,
  signal: AbortSignal,
): Promise<ReadableStreamReadResult<Uint8Array>> {
  if (signal.aborted) {
    return Promise.reject(new ModelManagerError('CANCELLED', 'Model download was cancelled.'));
  }
  return new Promise((resolveRead, reject) => {
    let settled = false;
    const finish = (operation: () => void) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      signal.removeEventListener('abort', onAbort);
      operation();
    };
    const onAbort = () =>
      finish(() => reject(new ModelManagerError('CANCELLED', 'Model download was cancelled.')));
    const timer = setTimeout(
      () =>
        finish(() =>
          reject(new ModelManagerError('TIMEOUT', 'Model download became inactive.', true)),
        ),
      timeoutMs,
    );
    timer.unref();
    signal.addEventListener('abort', onAbort, { once: true });
    void reader.read().then(
      (result) => finish(() => resolveRead(result)),
      (error: unknown) =>
        finish(() =>
          reject(error instanceof Error ? error : new Error('Model response body failed.')),
        ),
    );
  });
}

async function cancelResponseBody(response: Response): Promise<void> {
  await response.body?.cancel().catch(() => undefined);
}

async function publishStagedFile(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  await renameWithWindowsRetry(source, target, renameOperation);
}

async function publishRevisionDirectory(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  await recoverRevisionDirectory(target);
  const backup = `${target}.${randomUUID()}.replaced`;
  let movedExisting = false;
  try {
    try {
      await renameWithWindowsRetry(target, backup, renameOperation);
      movedExisting = true;
    } catch (error: unknown) {
      if (!hasCode(error, 'ENOENT')) throw error;
    }
    await renameWithWindowsRetry(source, target, renameOperation);
  } catch (error: unknown) {
    await renameWithWindowsRetry(backup, target, renameOperation).catch(() => undefined);
    throw error;
  }
  if (movedExisting) {
    await rm(backup, { recursive: true, force: true }).catch(() => undefined);
  }
}

async function publishAtomically(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  if (process.platform !== 'win32') {
    await renameOperation(source, target);
    return;
  }
  await recoverPublicationTarget(target);
  const backup = `${target}.${randomUUID()}.replaced`;
  let movedExisting = false;
  try {
    try {
      await renameWithWindowsRetry(target, backup, renameOperation);
      movedExisting = true;
    } catch (error: unknown) {
      if (!hasCode(error, 'ENOENT')) throw error;
    }
    await renameWithWindowsRetry(source, target, renameOperation);
  } catch (error: unknown) {
    await renameWithWindowsRetry(backup, target, renameOperation).catch(() => undefined);
    throw error;
  }
  // Publication already succeeded. A scanner holding the obsolete backup must not turn success
  // into a false setup failure; startup recovery safely removes it later.
  if (movedExisting) await rm(backup, { force: true }).catch(() => undefined);
}

async function renameWithWindowsRetry(
  source: string,
  target: string,
  renameOperation: typeof rename,
): Promise<void> {
  const attempts = process.platform === 'win32' ? 7 : 1;
  let lastError: unknown;
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    try {
      await renameOperation(source, target);
      return;
    } catch (error: unknown) {
      lastError = error;
      if (!isTransientWindowsFileError(error) || attempt + 1 === attempts) throw error;
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 25 * 2 ** attempt));
    }
  }
  throw lastError;
}

async function recoverRevisionDirectory(target: string): Promise<void> {
  const directory = dirname(target);
  const name = basename(target);
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return;
    throw error;
  }
  const backups = entries
    .filter(
      (entry) =>
        entry.name === `${name}.replaced` ||
        (entry.name.startsWith(`${name}.`) && entry.name.endsWith('.replaced')),
    )
    .map((entry) => join(directory, entry.name))
    .sort();
  if (backups.length === 0) return;
  const targetMetadata = await safeLstatForRecovery(target);
  if (targetMetadata !== null) {
    await Promise.all(
      backups.map((backup) => rm(backup, { recursive: true, force: true }).catch(() => undefined)),
    );
    return;
  }
  let restored = false;
  for (const backup of backups) {
    const metadata = await safeLstatForRecovery(backup);
    if (!restored && metadata?.isDirectory() === true && !metadata.isSymbolicLink()) {
      await rename(backup, target);
      restored = true;
    } else {
      await rm(backup, { recursive: true, force: true });
    }
  }
}

async function recoverPublicationTarget(target: string): Promise<void> {
  const directory = dirname(target);
  const name = basename(target);
  let entries;
  try {
    entries = await readdir(directory, { withFileTypes: true });
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return;
    throw error;
  }
  const backups = entries
    .filter(
      (entry) =>
        entry.name === `${name}.replaced` ||
        (entry.name.startsWith(`${name}.`) && entry.name.endsWith('.replaced')),
    )
    .map((entry) => join(directory, entry.name))
    .sort();
  if (backups.length === 0) return;
  const targetMetadata = await safeLstatForRecovery(target);
  if (targetMetadata !== null) {
    await Promise.all(
      backups.map((backup) => rm(backup, { recursive: true, force: true }).catch(() => undefined)),
    );
    return;
  }
  let restored = false;
  for (const backup of backups) {
    const metadata = await safeLstatForRecovery(backup);
    if (!restored && metadata?.isFile() === true && !metadata.isSymbolicLink()) {
      await rename(backup, target);
      restored = true;
    } else {
      await rm(backup, { recursive: true, force: true });
    }
  }
}

async function safeLstatForRecovery(path: string): Promise<Stats | null> {
  try {
    return await lstat(path);
  } catch (error: unknown) {
    if (hasCode(error, 'ENOENT')) return null;
    throw error;
  }
}

function sameOpenedFile(first: Stats, second: Stats): boolean {
  return (
    first.size === second.size &&
    first.mtimeMs === second.mtimeMs &&
    first.ctimeMs === second.ctimeMs &&
    first.birthtimeMs === second.birthtimeMs &&
    first.dev === second.dev &&
    first.ino === second.ino
  );
}

function validUnsatisfiedRange(response: Response, total: number): boolean {
  if (response.status !== 416) return false;
  const match = /^bytes \*\/(\d+)$/.exec(response.headers.get('content-range') ?? '');
  return match !== null && Number(match[1]) === total;
}

function validContentRange(response: Response, offset: number, total: number): boolean {
  if (response.status !== 206) return false;
  const match = /^bytes (\d+)-(\d+)\/(\d+)$/.exec(response.headers.get('content-range') ?? '');
  return (
    match !== null &&
    Number(match[1]) === offset &&
    Number(match[3]) === total &&
    Number(match[2]) === total - 1
  );
}

async function defaultAvailableBytes(path: string): Promise<number> {
  const values = await statfs(path);
  const available = BigInt(values.bavail) * BigInt(values.bsize);
  return available > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(available);
}

function defaultModelUrl(model: ModelManifestEntry, file: ModelManifestFile): string {
  const path = file.path.split('/').map(encodeURIComponent).join('/');
  return `https://huggingface.co/${model.id}/resolve/${model.revision}/${path}`;
}

function defaultValidateRequestUrl(value: string): boolean {
  try {
    const url = new URL(value);
    return (
      url.protocol === 'https:' &&
      (url.hostname === 'huggingface.co' ||
        url.hostname === 'hf.co' ||
        url.hostname.endsWith('.hf.co') ||
        url.hostname.endsWith('.huggingface.co') ||
        url.hostname.endsWith('.xethub.hf.co'))
    );
  } catch {
    return false;
  }
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
  if (
    ['ENETUNREACH', 'ENOTFOUND', 'ECONNREFUSED', 'EAI_AGAIN'].includes(code) ||
    error instanceof TypeError
  ) {
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
