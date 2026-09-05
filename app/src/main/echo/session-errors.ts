import { CaptureClientError } from '../audio/capture-window-client';
import { HelperClientError } from '../helper/helper-client-error';
import { ProviderError } from '../providers/errors';
import { ModelManagerError, WhisperClientError } from '../transcription/errors';

/** Operational codes only; exception messages can contain user text or paths. */
export function sessionFailureCode(error: unknown): string {
  if (error instanceof CaptureClientError) return `capture:${error.code}`;
  if (error instanceof HelperClientError) return `helper:${error.code}`;
  if (error instanceof WhisperClientError) return `speech:${error.code}`;
  if (error instanceof ModelManagerError) return `model:${error.code}`;
  if (error instanceof ProviderError) return `provider:${error.code}`;
  if (error instanceof AggregateError) return 'multiple-operations';
  if (error instanceof TypeError) return 'invalid-operation';
  if (error instanceof RangeError) return 'invalid-range';
  return 'internal';
}

export function publicSessionError(error: unknown): string {
  console.error('Dictation operation failed:', sessionFailureCode(error));
  if (error instanceof CaptureClientError && error.code === 'device-unavailable') {
    return 'Your selected microphone is unavailable. Choose another microphone in Settings.';
  }
  return 'Dictation could not be completed.';
}
