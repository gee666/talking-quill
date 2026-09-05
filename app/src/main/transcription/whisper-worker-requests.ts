import { randomUUID } from 'node:crypto';
import {
  WhisperWorkerRequestSchema,
  WhisperWorkerResponseSchema,
  type WhisperWorkerRequest,
  type WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';
import type {
  WorkerProcess,
  PendingRequest,
  WorkerRequestOptions,
  TerminationIntent,
} from './whisper-worker-contracts';
import type { WhisperWorkerProcess } from './whisper-worker-process';
import {
  createRequestDeadline,
  requestQueueTimeoutError,
  waitForDispatchTurn,
} from './whisper-worker-requests-support';

/** Serial dispatch, response validation, and generation-scoped request settlement. */
export class WhisperWorkerRequests {
  readonly #worker: WhisperWorkerProcess;
  readonly #pending = new Map<string, PendingRequest>();
  #dispatchTail: Promise<void> = Promise.resolve();

  constructor(worker: WhisperWorkerProcess) {
    this.#worker = worker;
  }

  async waitForTermination(signal?: AbortSignal): Promise<void> {
    const termination = this.#worker.termination.current;
    if (termination !== null) await waitForDispatchTurn(termination.settled, signal);
  }

  async request(
    create: (requestId: string) => WhisperWorkerRequest,
    options: WorkerRequestOptions,
  ): Promise<WhisperWorkerResult> {
    const deadline = createRequestDeadline(options.timeoutMs, options.signal);
    let dispatched = false;
    let releaseTurn = (): void => undefined;
    try {
      await this.waitForTermination(deadline.signal);
      this.#assertRequestAllowed(options);
      if (this.#worker.closing && this.#worker.process === null) {
        throw new WhisperClientError('CANCELLED', 'Whisper worker is closing.');
      }
      let process: WorkerProcess;
      try {
        process = this.#worker.ensureProcess();
      } catch {
        this.#worker.scheduleRestart();
        throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker could not start.');
      }
      const generation = this.#worker.generation;
      const requestId = randomUUID();
      options.captureRequestId?.(requestId);
      const request = WhisperWorkerRequestSchema.parse(create(requestId));

      const precedingRequest = this.#dispatchTail;
      const turn = new Promise<void>((resolve) => {
        releaseTurn = resolve;
      });
      this.#dispatchTail = precedingRequest.catch(() => undefined).then(() => turn);
      await waitForDispatchTurn(precedingRequest, deadline.signal);
      if (deadline.timedOut()) throw requestQueueTimeoutError();
      this.#assertRequestAllowed(options, process, generation);
      options.captureGeneration?.(generation);
      dispatched = true;
      return await this.#dispatchRequest(
        process,
        generation,
        requestId,
        request,
        options,
        deadline.signal,
      );
    } catch (error: unknown) {
      if (!dispatched && deadline.timedOut() && options.signal?.aborted !== true) {
        throw requestQueueTimeoutError();
      }
      throw error;
    } finally {
      releaseTurn();
      deadline.dispose();
    }
  }

  #assertRequestAllowed(
    options: WorkerRequestOptions,
    process?: WorkerProcess,
    generation?: number,
  ): void {
    if (options.signal?.aborted === true) {
      throw new WhisperClientError('CANCELLED', 'Transcription was cancelled.');
    }
    if (this.#worker.closing && options.allowClosing !== true) {
      throw new WhisperClientError('CANCELLED', 'Whisper worker is closing.');
    }
    if (
      process !== undefined &&
      generation !== undefined &&
      (this.#worker.process !== process ||
        this.#worker.generation !== generation ||
        this.#worker.termination.current !== null)
    ) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker generation changed.');
    }
    if (this.#worker.termination.current !== null) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker is terminating.');
    }
    if (
      options.expectedGeneration !== undefined &&
      (this.#worker.generation !== options.expectedGeneration || this.#worker.process === null)
    ) {
      throw new WhisperClientError('WORKER_CRASHED', 'Streaming worker generation changed.');
    }
  }

  #dispatchRequest(
    process: WorkerProcess,
    generation: number,
    requestId: string,
    request: WhisperWorkerRequest,
    options: WorkerRequestOptions,
    requestSignal: AbortSignal,
  ): Promise<WhisperWorkerResult> {
    options.onDispatched?.(requestId);
    return new Promise((resolve, reject) => {
      const cleanup = (): void => {
        requestSignal.removeEventListener('abort', onAbort);
      };
      const onAbort = () => {
        const callerCancelled = options.signal?.aborted === true;
        void this.#worker.beginTermination(
          generation,
          callerCancelled ? 'cancel' : 'health',
          callerCancelled
            ? new WhisperClientError('CANCELLED', 'Transcription was cancelled.')
            : new WhisperClientError('WORKER_CRASHED', 'Whisper worker request timed out.'),
          !callerCancelled,
          new Set([requestId]),
        );
      };
      this.#pending.set(requestId, {
        generation,
        accepts: options.accepts ?? (() => true),
        resolve: (result) => {
          cleanup();
          resolve(result);
        },
        reject: (error) => {
          cleanup();
          reject(error);
        },
      });
      if (requestSignal.aborted) onAbort();
      else requestSignal.addEventListener('abort', onAbort, { once: true });
      try {
        process.postMessage(request);
      } catch {
        void this.#worker.beginTermination(
          generation,
          'unavailable',
          new WhisperClientError('WORKER_CRASHED', 'Whisper worker was unavailable.'),
          true,
        );
      }
    });
  }

  handleMessage(generation: number, raw: unknown): void {
    if (
      generation !== this.#worker.generation ||
      this.#worker.lastExitedGeneration === generation ||
      this.#worker.termination.current?.generation === generation
    ) {
      return;
    }
    const response = WhisperWorkerResponseSchema.safeParse(raw);
    if (!response.success) {
      void this.#worker.beginTermination(
        generation,
        'protocol',
        new WhisperClientError('PROTOCOL_ERROR', 'Whisper worker sent an invalid response.'),
        true,
      );
      return;
    }
    if (
      response.data.requestId === 'worker-ready' &&
      response.data.ok &&
      response.data.result.type === 'ready'
    ) {
      this.#worker.markHealthy(generation);
      return;
    }
    if (!response.data.ok && response.data.error.code === 'WORKER_CRASHED') {
      void this.#worker.beginTermination(
        generation,
        'unavailable',
        new WhisperClientError('WORKER_CRASHED', response.data.error.message),
        true,
      );
      return;
    }
    const pending = this.#pending.get(response.data.requestId);
    if (pending?.generation !== generation) return;
    if (response.data.ok && !pending.accepts(response.data.result)) {
      void this.#worker.beginTermination(
        generation,
        'protocol',
        new WhisperClientError('PROTOCOL_ERROR', 'Whisper worker returned the wrong response.'),
        true,
      );
      return;
    }
    this.#pending.delete(response.data.requestId);
    if (response.data.ok) pending.resolve(response.data.result);
    else {
      pending.reject(new WhisperClientError(response.data.error.code, response.data.error.message));
    }
  }

  rejectGeneration(
    generation: number,
    termination: TerminationIntent | null,
    fallbackError: WhisperClientError,
  ): void {
    for (const [id, pending] of this.#pending) {
      if (pending.generation !== generation) continue;
      this.#pending.delete(id);
      const collateralCancellation =
        termination?.kind === 'cancel' &&
        termination.cancelledRequestIds !== null &&
        !termination.cancelledRequestIds.has(id);
      pending.reject(
        collateralCancellation
          ? new WhisperClientError(
              'WORKER_CRASHED',
              'Whisper worker stopped for another cancelled operation.',
            )
          : (termination?.error ?? fallbackError),
      );
    }
  }

  rejectPending(error: Error): void {
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }
}
