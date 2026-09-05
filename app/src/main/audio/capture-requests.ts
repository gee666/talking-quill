import type { MessagePortMain } from 'electron';
import { CAPTURE_COMMAND_TIMEOUT_MS } from '../../shared/constants/audio';
import type { CapturePortCommand, CapturePortMessage } from '../../shared/ipc/capture-port';
import { CaptureClientError } from './capture-client-error';

interface PendingRequest {
  readonly resolve: (message: CapturePortMessage) => void;
  readonly reject: (error: CaptureClientError) => void;
  readonly timer: ReturnType<typeof setTimeout>;
  readonly removeAbortListener: () => void;
}

/** Owns command deadlines and abort listeners, not the capture port. */
export class CaptureRequests {
  readonly #pending = new Map<string, PendingRequest>();
  request(
    port: MessagePortMain | null,
    closePort: () => void,
    command: CapturePortCommand,
    signal?: AbortSignal,
  ): Promise<CapturePortMessage> {
    if (port === null || signal?.aborted === true) {
      return Promise.reject(new CaptureClientError('capture-unavailable'));
    }
    return new Promise((resolve, reject) => {
      let removeAbortListener: () => void = () => undefined;
      const timer = setTimeout(() => {
        this.#pending.delete(command.requestId);
        removeAbortListener();
        reject(new CaptureClientError('capture-unavailable'));
      }, CAPTURE_COMMAND_TIMEOUT_MS);
      timer.unref();
      if (signal !== undefined) {
        const abort = () => closePort();
        signal.addEventListener('abort', abort, { once: true });
        removeAbortListener = () => signal.removeEventListener('abort', abort);
      }
      this.#pending.set(command.requestId, {
        resolve,
        reject,
        timer,
        removeAbortListener: () => removeAbortListener(),
      });
      if (signal?.aborted === true) {
        closePort();
        return;
      }
      try {
        port.postMessage(command);
      } catch {
        clearTimeout(timer);
        this.#pending.delete(command.requestId);
        removeAbortListener();
        reject(new CaptureClientError('capture-unavailable'));
      }
    });
  }

  settle(message: CapturePortMessage & { requestId: string | null }): void {
    const requestId = message.requestId;
    if (requestId === null) return;
    const pending = this.#pending.get(requestId);
    if (pending === undefined) return;
    this.#pending.delete(requestId);
    clearTimeout(pending.timer);
    pending.removeAbortListener();
    if (message.type === 'request:error') {
      pending.reject(new CaptureClientError(message.code));
    } else {
      pending.resolve(message);
    }
  }

  rejectAll(): void {
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.removeAbortListener();
      pending.reject(new CaptureClientError('capture-unavailable'));
    }
    this.#pending.clear();
  }
}
