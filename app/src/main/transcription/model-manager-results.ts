import type { ModelManifestEntry } from '../../shared/schemas/model-manifest';
import {
  ModelStatusSchema,
  type ModelState,
  type ModelStatus,
} from '../../shared/schemas/transcription';
import { ModelManagerError, WhisperClientError } from './errors';

export function makeStatus(
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

export function shuttingDownError(): ModelManagerError {
  return new ModelManagerError('CANCELLED', 'Model manager is shutting down.');
}

export function waitForModelTask<Value>(
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

export function mapDownloadError(
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
