import type {
  WhisperAcknowledgedOperation,
  WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';

export const CONTROL_REQUEST_TIMEOUT_MS = 30_000;

export function acceptsAcknowledgement(
  operation: WhisperAcknowledgedOperation,
): (result: WhisperWorkerResult) => boolean {
  return (result) => result.type === 'acknowledged' && result.operation === operation;
}

export function assertAcknowledged(
  result: WhisperWorkerResult,
  operation: WhisperAcknowledgedOperation,
): void {
  if (result.type !== 'acknowledged' || result.operation !== operation) {
    throw new WhisperClientError(
      'PROTOCOL_ERROR',
      `Whisper worker returned the wrong acknowledgement for ${operation}.`,
    );
  }
}

export function createRequestDeadline(
  timeoutMs: number,
  callerSignal: AbortSignal | undefined,
): {
  readonly signal: AbortSignal;
  readonly timedOut: () => boolean;
  readonly dispose: () => void;
} {
  const timeoutController = new AbortController();
  const timeout = setTimeout(() => timeoutController.abort('worker request timeout'), timeoutMs);
  timeout.unref();
  return {
    signal:
      callerSignal === undefined
        ? timeoutController.signal
        : AbortSignal.any([callerSignal, timeoutController.signal]),
    timedOut: () => timeoutController.signal.aborted,
    dispose: () => clearTimeout(timeout),
  };
}

export function requestQueueTimeoutError(): WhisperClientError {
  return new WhisperClientError(
    'WORKER_CRASHED',
    'Whisper worker request timed out while waiting for dispatch.',
  );
}

export function waitForDispatchTurn(
  precedingRequest: Promise<void>,
  signal: AbortSignal | undefined,
): Promise<void> {
  if (signal?.aborted === true) {
    return Promise.reject(new WhisperClientError('CANCELLED', 'Transcription was cancelled.'));
  }
  if (signal === undefined) return precedingRequest;
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (operation: () => void): void => {
      if (settled) return;
      settled = true;
      signal.removeEventListener('abort', abort);
      operation();
    };
    const abort = (): void =>
      finish(() => reject(new WhisperClientError('CANCELLED', 'Transcription was cancelled.')));
    signal.addEventListener('abort', abort, { once: true });
    void precedingRequest.then(
      () => finish(resolve),
      () => finish(resolve),
    );
  });
}

export function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, milliseconds);
    timer.unref();
  });
}
