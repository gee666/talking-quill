import { WHISPER_PROTOCOL_VERSION } from '../../shared/constants/whisper';
import type {
  WhisperWorkerRequest,
  WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';
import type {
  WhisperWorkerSupervisorOptions,
  WorkerRequestOptions,
  TerminationKind,
} from './whisper-worker-contracts';
import { WhisperWorkerProcess } from './whisper-worker-process';
import { WhisperWorkerRequests } from './whisper-worker-requests';
import {
  CONTROL_REQUEST_TIMEOUT_MS,
  acceptsAcknowledgement,
  assertAcknowledged,
  delay,
} from './whisper-worker-requests-support';

export { CONTROL_REQUEST_TIMEOUT_MS } from './whisper-worker-requests-support';
export type { WhisperWorkerSpawner, WorkerRequestOptions } from './whisper-worker-contracts';

const SHUTDOWN_GRACE_MS = 1_000;

/** Coordinates worker requests, generation lifetime, and graceful close. */
export class WhisperWorkerSupervisor {
  readonly #worker: WhisperWorkerProcess;
  readonly #requests: WhisperWorkerRequests;
  #closePromise: Promise<void> | null = null;

  constructor(options: WhisperWorkerSupervisorOptions) {
    this.#worker = new WhisperWorkerProcess(
      options,
      (generation, message) => this.#requests.handleMessage(generation, message),
      (generation, termination, error) =>
        this.#requests.rejectGeneration(generation, termination, error),
    );
    this.#requests = new WhisperWorkerRequests(this.#worker);
  }

  request(
    create: (requestId: string) => WhisperWorkerRequest,
    options: WorkerRequestOptions,
  ): Promise<WhisperWorkerResult> {
    return this.#requests.request(create, options);
  }

  beginTermination(
    generation: number,
    kind: TerminationKind,
    error: WhisperClientError,
    restart: boolean,
    cancelledRequestIds: ReadonlySet<string> | null = null,
  ): Promise<void> {
    return this.#worker.beginTermination(generation, kind, error, restart, cancelledRequestIds);
  }

  async waitForTermination(signal?: AbortSignal): Promise<void> {
    await this.#requests.waitForTermination(signal);
  }

  releaseUseWhenSafe(generation: number, release: () => void): void {
    this.#worker.termination.releaseUseWhenSafe(generation, release);
  }

  registerSessionLease(generation: number, release: () => void): void {
    this.#worker.termination.registerSessionLease(generation, release);
  }

  unregisterSessionLease(generation: number, release: () => void): void {
    this.#worker.termination.unregisterSessionLease(generation, release);
  }

  captureActiveGeneration(): number | undefined {
    return this.#worker.process === null ? undefined : this.#worker.generation;
  }

  hasProcess(): boolean {
    return this.#worker.process !== null;
  }

  isCurrentGeneration(generation: number): boolean {
    return this.#worker.generation === generation && this.#worker.process !== null;
  }

  isOperationalGeneration(generation: number): boolean {
    return (
      this.#worker.generation === generation &&
      this.#worker.process !== null &&
      this.#worker.termination.current === null &&
      !this.#worker.closing
    );
  }

  close(): Promise<void> {
    this.#closePromise ??= this.#closeInternal();
    return this.#closePromise;
  }

  async #closeInternal(): Promise<void> {
    this.#worker.closing = true;
    this.#worker.clearSupervisionTimers();
    this.#worker.termination.retryRetiredProcessCleanup();
    const process = this.#worker.process;
    const generation = this.#worker.generation;
    const exited = this.#worker.generationExit;
    if (process === null || exited === null) {
      this.#requests.rejectPending(new WhisperClientError('CANCELLED', 'Whisper worker closed.'));
      return;
    }
    if (this.#worker.termination.current !== null) {
      await this.beginTermination(
        generation,
        'close',
        new WhisperClientError('CANCELLED', 'Whisper worker closed.'),
        false,
      );
      return;
    }
    const shutdownState: { protocolError: WhisperClientError | null } = {
      protocolError: null,
    };
    const shutdown = this.request(
      (requestId) => ({ version: WHISPER_PROTOCOL_VERSION, requestId, type: 'shutdown' }),
      {
        timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
        allowClosing: true,
        expectedGeneration: generation,
        accepts: acceptsAcknowledgement('shutdown'),
      },
    )
      .then((result) => assertAcknowledged(result, 'shutdown'))
      .catch((error: unknown) => {
        if (error instanceof WhisperClientError && error.code === 'PROTOCOL_ERROR') {
          shutdownState.protocolError = error;
        }
      });
    await Promise.race([shutdown, delay(SHUTDOWN_GRACE_MS)]);
    await Promise.race([exited, delay(100)]);
    if (this.#worker.process !== null && this.#worker.generation === generation) {
      await this.beginTermination(
        generation,
        'close',
        new WhisperClientError('CANCELLED', 'Whisper worker closed.'),
        false,
      );
    } else {
      await exited;
    }
    if (shutdownState.protocolError !== null) throw shutdownState.protocolError;
  }
}
