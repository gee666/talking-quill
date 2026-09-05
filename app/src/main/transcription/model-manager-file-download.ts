import type {
  ModelManifestEntry,
  ModelManifestFile,
  VerifiedModelFileIdentity,
} from '../../shared/schemas/model-manifest';
import { ModelDownloadTransport, type ModelDownloadResponse } from './model-download-transport';
import { ModelManagerError } from './errors';
import type { ModelManagerContext } from './model-manager-context';
import type { ActiveDownload, ModelManagerOptions } from './model-manager-types';

/** Resumes one partial file and publishes it only after verification. */
export class ModelManagerFileDownload {
  readonly #context: ModelManagerContext;
  readonly #transport: ModelDownloadTransport;

  constructor(context: ModelManagerContext, options: ModelManagerOptions) {
    this.#context = context;
    this.#transport = new ModelDownloadTransport(options);
  }

  async download(
    model: ModelManifestEntry,
    file: ModelManifestFile,
    completedBeforeFile: number,
    active: ActiveDownload,
  ): Promise<number> {
    await this.#context.repository.preparePartial(model, file);
    let offset = await this.#context.repository.partialSize(model, file);
    let verifiedPartIdentity: Omit<VerifiedModelFileIdentity, 'path'> | null = null;
    if (offset > file.size) {
      await this.#context.repository.removePartial(model, file);
      offset = 0;
    }
    if (offset === file.size) {
      const inspection = await this.#context.repository.inspectPartial(
        model,
        file,
        active.controller.signal,
      );
      if (inspection.valid && inspection.identity !== null) {
        verifiedPartIdentity = inspection.identity;
      } else {
        await this.#context.repository.removePartial(model, file);
        offset = 0;
      }
    }
    if (offset < file.size) {
      const response = await this.#transport.request(model, file, offset, active.controller.signal);
      let completedByConcurrentWriter = false;
      if (offset > 0 && validUnsatisfiedRange(response, file.size)) {
        await response.cancel();
        const currentSize = await this.#context.repository.partialSize(model, file);
        const inspection =
          currentSize === file.size
            ? await this.#context.repository.inspectPartial(model, file, active.controller.signal)
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
          await this.#context.repository.removePartial(model, file);
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
          writer = await this.#context.repository.openPartialWriter(model, file, offset);
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
            this.#context.emit(model, 'downloading', file, completedBeforeFile + written, written);
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
    await this.#context.repository.publishVerifiedPartial(
      model,
      file,
      verifiedPartIdentity,
      active.controller.signal,
    );
    this.#context.emit(model, 'downloading', file, completedBeforeFile + file.size, file.size);
    return completedBeforeFile + file.size;
  }
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
