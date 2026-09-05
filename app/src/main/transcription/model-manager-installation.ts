import type { ModelManifestEntry, WhisperModelId } from '../../shared/schemas/model-manifest';
import type { ModelState, ModelStatus } from '../../shared/schemas/transcription';
import { ModelManagerError } from './errors';
import type { ModelManagerContext } from './model-manager-context';
import { ModelManagerFileDownload } from './model-manager-file-download';
import type { ActiveDownload, ModelManagerOptions } from './model-manager-types';
import { makeStatus, mapDownloadError } from './model-manager-results';

/** Owns staged verification, publication, and installation outcomes. */
export class ModelManagerInstallation {
  readonly #context: ModelManagerContext;
  readonly #files: ModelManagerFileDownload;

  constructor(context: ModelManagerContext, options: ModelManagerOptions) {
    this.#context = context;
    this.#files = new ModelManagerFileDownload(context, options);
  }

  async download(modelId: WhisperModelId, active: ActiveDownload): Promise<ModelStatus> {
    const model = this.#context.getModel(modelId);
    this.#context.states.delete(modelId);
    try {
      await this.#context.repository.prepareRoots();
      const installed = await this.#context.repository.inspectInstalled(
        model,
        active.controller.signal,
        true,
      );
      if (installed.valid) {
        active.state = 'verifying';
        this.#context.emit(model, 'verifying', null, model.totalBytes);
        await this.#context.afterInstallValidation?.(model.id, active.controller.signal);
        active.controller.signal.throwIfAborted();
        await this.#context.repository.commitVerification(model, installed.identities);
        this.#context.emit(model, 'ready', null, model.totalBytes);
        return makeStatus(model, 'ready', model.totalBytes, null, false);
      }

      await this.#context.repository.prepareStaging(model);
      const stagedHardLinksInstalledFiles = await this.#context.repository.reuseInstalledFiles(
        model,
        installed.identities,
        active.controller.signal,
      );
      const staged = await this.#context.repository.inspectStaging(
        model,
        active.controller.signal,
        true,
      );
      let verified = staged;
      const completeStagingStillCurrent =
        staged.valid &&
        (await this.#context.repository.stagedIdentityStillCurrent(model, staged.identities));
      if (!completeStagingStillCurrent) {
        await this.#context.repository.ensureDownloadCapacity(model, staged.identities);
        let completed = 0;
        for (const file of model.files) {
          active.controller.signal.throwIfAborted();
          if (
            (
              await this.#context.repository.inspectStagedFile(
                model,
                file,
                active.controller.signal,
                true,
              )
            ).valid
          ) {
            await this.#context.repository.removePartial(model, file);
            completed += file.size;
            this.#context.emit(model, 'downloading', file, completed, file.size);
            continue;
          }
          await this.#context.repository.removeStagedFile(model, file);
          completed = await this.#files.download(model, file, completed, active);
        }
        active.state = 'verifying';
        this.#context.emit(model, 'verifying', null, model.totalBytes);
        verified = await this.#context.repository.inspectStaging(
          model,
          active.controller.signal,
          true,
        );
        if (!verified.valid) {
          throw new ModelManagerError(
            'CORRUPT',
            'Downloaded staged model failed verification.',
            true,
          );
        }
      } else {
        active.state = 'verifying';
        this.#context.emit(model, 'verifying', null, model.totalBytes);
      }
      await this.#context.repository.prepareStagedPublication(model, verified.identities);
      active.controller.signal.throwIfAborted();
      active.state = 'installing';
      this.#context.emit(model, 'installing', null, model.totalBytes);
      await this.#context.repository.publishStagedRevision(model);
      try {
        await this.#context.repository.assertPublishedManifestEntries(model);
      } catch (error: unknown) {
        await this.#context.repository.removeInstalledRevision(model);
        throw error;
      }
      let installedIdentities = verified.identities;
      if (stagedHardLinksInstalledFiles) {
        const published = await this.#context.repository.inspectInstalled(
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
        !(await this.#context.repository.installedIdentityStillCurrent(model, installedIdentities))
      ) {
        throw new ModelManagerError(
          'CORRUPT',
          'Published model identity changed during installation.',
          true,
        );
      }
      await this.#context.afterInstallValidation?.(model.id, active.controller.signal);
      active.controller.signal.throwIfAborted();
      await this.#context.repository.commitVerification(model, installedIdentities);
      this.#context.states.delete(modelId);
      this.#context.emit(model, 'ready', null, model.totalBytes);
      return makeStatus(model, 'ready', model.totalBytes, null, false);
    } catch (error: unknown) {
      if (active.controller.signal.aborted) {
        return this.finishCancelledDownload(model, active);
      }
      const mapped = mapDownloadError(error, active.state);
      if (mapped.code === 'WORKER_VALIDATION') {
        await this.#context.repository.removeCompletionMarker(model).catch(() => undefined);
      }
      const state: ModelState =
        mapped.code === 'OFFLINE' ? 'offline' : mapped.code === 'CORRUPT' ? 'corrupt' : 'error';
      this.#context.states.set(modelId, {
        state,
        detail: mapped.message,
        repairable: mapped.repairable,
      });
      const status = await this.#context.statusFromDisk(
        model,
        state,
        mapped.message,
        mapped.repairable,
        false,
      );
      this.#context.emit(model, state, null, status.downloadedBytes);
      throw mapped;
    }
  }

  async finishCancelledDownload(
    model: ModelManifestEntry,
    active: ActiveDownload,
  ): Promise<ModelStatus> {
    if (active.intent === 'external') {
      throw new ModelManagerError('CANCELLED', 'Model download was cancelled.');
    }
    const state: ModelState = active.intent === 'paused' ? 'paused' : 'missing';
    const detail = active.intent === 'paused' ? 'Download paused.' : 'Download cancelled.';
    this.#context.states.set(model.id, { state, detail, repairable: false });
    const status = await this.#context.statusFromDisk(model, state, detail, false, false);
    this.#context.emit(model, state, null, status.downloadedBytes);
    return status;
  }
}
