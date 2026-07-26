import { utilityProcess } from 'electron';
import { randomUUID } from 'node:crypto';
import { join } from 'node:path';
import { kill as forceKillProcess } from 'node:process';
import { WHISPER_PROTOCOL_VERSION } from '../../shared/constants/whisper';
import {
  WhisperWorkerRequestSchema,
  WhisperWorkerResponseSchema,
  type WhisperAcknowledgedOperation,
  type WhisperWorkerRequest,
  type WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';

const MAX_AUTOMATIC_RESTARTS = 5;
const MAX_RESTART_DELAY_MS = 2_000;
const HEALTH_TIMEOUT_MS = 5_000;
const STABILITY_RESET_MS = 30_000;
const SHUTDOWN_GRACE_MS = 1_000;
const FORCE_KILL_AFTER_MS = 1_500;
const FORCE_KILL_RETRY_MS = 1_500;
const TERMINATION_DEADLINE_MS = FORCE_KILL_AFTER_MS + FORCE_KILL_RETRY_MS + 1_500;
export const CONTROL_REQUEST_TIMEOUT_MS = 30_000;

interface WorkerProcess {
  readonly pid: number | undefined;
  postMessage(message: unknown): void;
  kill(): boolean;
  on(event: 'message', listener: (message: unknown) => void): this;
  on(event: 'exit', listener: (code: number) => void): this;
}

export type WhisperWorkerSpawner = (modulePath: string, args: readonly string[]) => WorkerProcess;

interface PendingRequest {
  readonly generation: number;
  readonly accepts: (result: WhisperWorkerResult) => boolean;
  readonly resolve: (result: WhisperWorkerResult) => void;
  readonly reject: (error: Error) => void;
}

export interface WorkerRequestOptions {
  readonly timeoutMs: number;
  readonly signal?: AbortSignal | undefined;
  readonly allowClosing?: boolean;
  readonly expectedGeneration?: number | undefined;
  readonly accepts?: (result: WhisperWorkerResult) => boolean;
  readonly captureGeneration?: (generation: number) => void;
  readonly captureRequestId?: (requestId: string) => void;
  readonly onDispatched?: (requestId: string) => void;
}

type TerminationKind = 'cancel' | 'close' | 'health' | 'protocol' | 'unavailable';

interface TerminationIntent {
  readonly generation: number;
  kind: TerminationKind;
  error: WhisperClientError;
  restart: boolean;
  readonly settled: Promise<void>;
  readonly cancelledRequestIds: ReadonlySet<string> | null;
  forceTimer: ReturnType<typeof setTimeout> | null;
  retryTimer: ReturnType<typeof setTimeout> | null;
  deadlineTimer: ReturnType<typeof setTimeout> | null;
}

export class WhisperWorkerSupervisor {
  readonly #cacheDirectory: string;
  readonly #workerPath: string;
  readonly #spawn: WhisperWorkerSpawner;
  readonly #forceKill: (pid: number) => void;
  readonly #pending = new Map<string, PendingRequest>();
  readonly #sessionLeaseReleases = new Map<number, Set<() => void>>();
  #dispatchTail: Promise<void> = Promise.resolve();
  #process: WorkerProcess | null = null;
  #generation = 0;
  #lastExitedGeneration = 0;
  #healthyGeneration = 0;
  #generationExit: Promise<void> | null = null;
  #resolveGenerationExit: (() => void) | null = null;
  #termination: TerminationIntent | null = null;
  #closing = false;
  #unusable = false;
  #closePromise: Promise<void> | null = null;
  #restartAttempts = 0;
  #restartTimer: ReturnType<typeof setTimeout> | null = null;
  #healthTimer: ReturnType<typeof setTimeout> | null = null;
  #stabilityTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: {
    readonly cacheDirectory: string;
    readonly workerPath?: string | undefined;
    readonly spawn?: WhisperWorkerSpawner | undefined;
    readonly forceKill?: ((pid: number) => void) | undefined;
  }) {
    this.#cacheDirectory = options.cacheDirectory;
    this.#workerPath =
      options.workerPath ?? join(__dirname, '..', 'workers', 'whisper-bootstrap.cjs');
    this.#spawn = options.spawn ?? defaultSpawner;
    this.#forceKill = options.forceKill ?? defaultForceKill;
  }

  captureActiveGeneration(): number | undefined {
    return this.#process === null ? undefined : this.#generation;
  }

  hasProcess(): boolean {
    return this.#process !== null;
  }

  isCurrentGeneration(generation: number): boolean {
    return this.#generation === generation && this.#process !== null;
  }

  isOperationalGeneration(generation: number): boolean {
    return (
      this.#generation === generation &&
      this.#process !== null &&
      this.#termination === null &&
      !this.#closing &&
      !this.#unusable
    );
  }

  async request(
    create: (requestId: string) => WhisperWorkerRequest,
    options: WorkerRequestOptions,
  ): Promise<WhisperWorkerResult> {
    const { signal } = options;
    await this.waitForTermination(options.signal);
    this.#assertRequestAllowed(options);
    if (this.#closing && this.#process === null) {
      throw new WhisperClientError('CANCELLED', 'Whisper worker is closing.');
    }
    let process: WorkerProcess;
    try {
      process = this.#ensureProcess();
    } catch {
      this.#scheduleRestart();
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker could not start.');
    }
    const generation = this.#generation;
    const requestId = randomUUID();
    options.captureRequestId?.(requestId);
    const request = WhisperWorkerRequestSchema.parse(create(requestId));

    const precedingRequest = this.#dispatchTail;
    let releaseTurn!: () => void;
    const turn = new Promise<void>((resolve) => {
      releaseTurn = resolve;
    });
    this.#dispatchTail = precedingRequest.catch(() => undefined).then(() => turn);
    try {
      await waitForDispatchTurn(precedingRequest, signal);
      this.#assertRequestAllowed(options, process, generation);
      options.captureGeneration?.(generation);
      return await this.#dispatchRequest(process, generation, requestId, request, options);
    } finally {
      releaseTurn();
    }
  }

  beginTermination(
    generation: number,
    kind: TerminationKind,
    error: WhisperClientError,
    restart: boolean,
    cancelledRequestIds: ReadonlySet<string> | null = null,
  ): Promise<void> {
    if (generation !== this.#generation || this.#process === null) return Promise.resolve();
    const existing = this.#termination;
    if (existing?.generation === generation) {
      if (kind === 'close') {
        existing.kind = kind;
        existing.error = error;
        existing.restart = false;
      }
      return existing.settled;
    }
    const process = this.#process;
    const generationExited = this.#generationExit ?? Promise.resolve();
    let resolveDeadline!: () => void;
    const deadlineReached = new Promise<void>((resolve) => {
      resolveDeadline = resolve;
    });
    const terminationSettled = Promise.race([generationExited, deadlineReached]);
    const termination: TerminationIntent = {
      generation,
      kind,
      error,
      restart,
      settled: terminationSettled,
      cancelledRequestIds,
      forceTimer: null,
      retryTimer: null,
      deadlineTimer: null,
    };
    this.#termination = termination;
    this.#tryKill(process);
    if (this.#termination !== termination || this.#process !== process) return terminationSettled;
    termination.forceTimer = setTimeout(() => {
      if (this.#termination !== termination || this.#process !== process) return;
      const pid = process.pid;
      if (pid === undefined) this.#tryKill(process);
      else {
        try {
          this.#forceKill(pid);
        } catch {
          this.#tryKill(process);
        }
      }
      termination.retryTimer = setTimeout(() => {
        if (this.#termination === termination && this.#process === process) {
          this.#tryKill(process);
        }
      }, FORCE_KILL_RETRY_MS);
      termination.retryTimer.unref();
    }, FORCE_KILL_AFTER_MS);
    termination.forceTimer.unref();
    termination.deadlineTimer = setTimeout(() => {
      if (this.#termination !== termination || this.#process !== process) return;
      this.#unusable = true;
      this.#rejectGeneration(generation, termination, termination.error, false);
      resolveDeadline();
    }, TERMINATION_DEADLINE_MS);
    termination.deadlineTimer.unref();
    return terminationSettled;
  }

  async waitForTermination(signal?: AbortSignal): Promise<void> {
    const termination = this.#termination;
    if (termination !== null) await waitForDispatchTurn(termination.settled, signal);
  }

  releaseUseWhenSafe(generation: number, release: () => void): void {
    if (
      generation > 0 &&
      this.#generation === generation &&
      this.#process !== null &&
      (this.#termination?.generation === generation || this.#unusable)
    ) {
      this.registerSessionLease(generation, release);
      return;
    }
    release();
  }

  registerSessionLease(generation: number, release: () => void): void {
    const releases = this.#sessionLeaseReleases.get(generation) ?? new Set<() => void>();
    releases.add(release);
    this.#sessionLeaseReleases.set(generation, releases);
  }

  unregisterSessionLease(generation: number, release: () => void): void {
    const releases = this.#sessionLeaseReleases.get(generation);
    releases?.delete(release);
    if (releases?.size === 0) this.#sessionLeaseReleases.delete(generation);
  }

  close(): Promise<void> {
    this.#closePromise ??= this.#closeInternal();
    return this.#closePromise;
  }

  async #closeInternal(): Promise<void> {
    this.#closing = true;
    this.#clearSupervisionTimers();
    if (this.#unusable) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker termination failed.');
    }
    const process = this.#process;
    const generation = this.#generation;
    const exited = this.#generationExit;
    if (process === null || exited === null) {
      this.#rejectPending(new WhisperClientError('CANCELLED', 'Whisper worker closed.'));
      return;
    }
    if (this.#termination !== null) {
      await this.beginTermination(
        generation,
        'close',
        new WhisperClientError('CANCELLED', 'Whisper worker closed.'),
        false,
      );
      this.#assertUsableTermination();
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
    if (this.#process !== null && this.#generation === generation) {
      await this.beginTermination(
        generation,
        'close',
        new WhisperClientError('CANCELLED', 'Whisper worker closed.'),
        false,
      );
    } else {
      await exited;
    }
    this.#assertUsableTermination();
    if (shutdownState.protocolError !== null) throw shutdownState.protocolError;
  }

  #assertUsableTermination(): void {
    if (this.#unusable) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker termination failed.');
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
    if (this.#unusable) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker could not be terminated.');
    }
    if (this.#closing && options.allowClosing !== true) {
      throw new WhisperClientError('CANCELLED', 'Whisper worker is closing.');
    }
    if (
      process !== undefined &&
      generation !== undefined &&
      (this.#process !== process || this.#generation !== generation || this.#termination !== null)
    ) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker generation changed.');
    }
    if (this.#termination !== null) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker is terminating.');
    }
    if (
      options.expectedGeneration !== undefined &&
      (this.#generation !== options.expectedGeneration || this.#process === null)
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
  ): Promise<WhisperWorkerResult> {
    const timeoutController = new AbortController();
    const timeout = setTimeout(
      () => timeoutController.abort('worker request timeout'),
      options.timeoutMs,
    );
    timeout.unref();
    const requestSignal =
      options.signal === undefined
        ? timeoutController.signal
        : AbortSignal.any([options.signal, timeoutController.signal]);
    options.onDispatched?.(requestId);
    return new Promise((resolve, reject) => {
      const cleanup = (): void => {
        clearTimeout(timeout);
        requestSignal.removeEventListener('abort', onAbort);
      };
      const onAbort = () => {
        const callerCancelled = options.signal?.aborted === true;
        void this.beginTermination(
          generation,
          callerCancelled ? 'cancel' : 'health',
          callerCancelled
            ? new WhisperClientError('CANCELLED', 'Transcription was cancelled.')
            : new WhisperClientError('WORKER_CRASHED', 'Whisper worker request timed out.'),
          !callerCancelled,
          new Set([requestId]),
        );
      };
      requestSignal.addEventListener('abort', onAbort, { once: true });
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
      try {
        process.postMessage(request);
      } catch {
        void this.beginTermination(
          generation,
          'unavailable',
          new WhisperClientError('WORKER_CRASHED', 'Whisper worker was unavailable.'),
          true,
        );
      }
    });
  }

  #ensureProcess(): WorkerProcess {
    if (this.#unusable) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker client is unusable.');
    }
    if (this.#termination !== null) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker is still terminating.');
    }
    if (this.#process !== null) return this.#process;
    this.#generation += 1;
    const generation = this.#generation;
    const process = this.#spawn(this.#workerPath, [`--model-cache=${this.#cacheDirectory}`]);
    this.#generationExit = new Promise<void>((resolve) => {
      this.#resolveGenerationExit = resolve;
    });
    process.on('message', (message) => this.#handleMessage(generation, message));
    process.on('exit', () => this.#handleExit(generation));
    this.#process = process;
    this.#healthTimer = setTimeout(() => {
      if (this.#healthyGeneration !== generation) {
        void this.beginTermination(
          generation,
          'health',
          new WhisperClientError('WORKER_CRASHED', 'Whisper worker health handshake timed out.'),
          true,
        );
      }
    }, HEALTH_TIMEOUT_MS);
    this.#healthTimer.unref();
    return process;
  }

  #handleMessage(generation: number, raw: unknown): void {
    if (generation !== this.#generation || this.#termination?.generation === generation) return;
    const response = WhisperWorkerResponseSchema.safeParse(raw);
    if (!response.success) {
      void this.beginTermination(
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
      this.#markHealthy(generation);
      return;
    }
    const pending = this.#pending.get(response.data.requestId);
    if (pending?.generation !== generation) return;
    if (response.data.ok && !pending.accepts(response.data.result)) {
      void this.beginTermination(
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

  #markHealthy(generation: number): void {
    if (generation !== this.#generation || this.#termination?.generation === generation) return;
    this.#healthyGeneration = generation;
    if (this.#healthTimer !== null) clearTimeout(this.#healthTimer);
    this.#healthTimer = null;
    if (this.#stabilityTimer !== null) clearTimeout(this.#stabilityTimer);
    this.#stabilityTimer = setTimeout(() => {
      if (this.#healthyGeneration === generation && this.#process !== null) {
        this.#restartAttempts = 0;
      }
    }, STABILITY_RESET_MS);
    this.#stabilityTimer.unref();
  }

  #handleExit(generation: number): void {
    if (generation !== this.#generation || this.#lastExitedGeneration === generation) return;
    this.#lastExitedGeneration = generation;
    const termination = this.#termination?.generation === generation ? this.#termination : null;
    this.#clearGenerationTimers(termination);
    this.#process = null;
    this.#healthyGeneration = 0;
    this.#termination = null;
    this.#resolveGenerationExit?.();
    this.#resolveGenerationExit = null;
    this.#generationExit = null;
    const fallbackError = this.#closing
      ? new WhisperClientError('CANCELLED', 'Whisper worker closed.')
      : new WhisperClientError('WORKER_CRASHED', 'Whisper worker exited unexpectedly.');
    this.#rejectGeneration(generation, termination, fallbackError);
    if (!this.#closing && (termination === null || termination.restart)) this.#scheduleRestart();
  }

  #tryKill(process: WorkerProcess): void {
    try {
      process.kill();
    } catch {
      // The bounded termination deadline handles a process API that keeps failing.
    }
  }

  #rejectGeneration(
    generation: number,
    termination: TerminationIntent | null,
    fallbackError: WhisperClientError,
    releaseLeases = true,
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
    const sessionReleases = releaseLeases ? this.#sessionLeaseReleases.get(generation) : undefined;
    if (sessionReleases !== undefined) {
      this.#sessionLeaseReleases.delete(generation);
      for (const release of sessionReleases) release();
    }
  }

  #scheduleRestart(): void {
    if (
      this.#restartTimer !== null ||
      this.#closing ||
      this.#unusable ||
      this.#restartAttempts >= MAX_AUTOMATIC_RESTARTS
    ) {
      return;
    }
    const delayMs = Math.min(MAX_RESTART_DELAY_MS, 100 * 2 ** this.#restartAttempts);
    this.#restartAttempts += 1;
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      if (
        !this.#closing &&
        !this.#unusable &&
        this.#process === null &&
        this.#termination === null
      ) {
        try {
          this.#ensureProcess();
        } catch {
          this.#scheduleRestart();
        }
      }
    }, delayMs);
    this.#restartTimer.unref();
  }

  #clearGenerationTimers(termination: TerminationIntent | null): void {
    for (const timer of [this.#healthTimer, this.#stabilityTimer]) {
      if (timer !== null) clearTimeout(timer);
    }
    this.#healthTimer = null;
    this.#stabilityTimer = null;
    if (termination !== null) {
      if (termination.forceTimer !== null) clearTimeout(termination.forceTimer);
      if (termination.retryTimer !== null) clearTimeout(termination.retryTimer);
      if (termination.deadlineTimer !== null) clearTimeout(termination.deadlineTimer);
    }
  }

  #clearSupervisionTimers(): void {
    for (const timer of [this.#restartTimer, this.#healthTimer, this.#stabilityTimer]) {
      if (timer !== null) clearTimeout(timer);
    }
    this.#restartTimer = null;
    this.#healthTimer = null;
    this.#stabilityTimer = null;
  }

  #rejectPending(error: Error): void {
    for (const pending of this.#pending.values()) pending.reject(error);
    this.#pending.clear();
  }
}

function acceptsAcknowledgement(
  operation: WhisperAcknowledgedOperation,
): (result: WhisperWorkerResult) => boolean {
  return (result) => result.type === 'acknowledged' && result.operation === operation;
}

function assertAcknowledged(
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

function waitForDispatchTurn(
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

function defaultSpawner(modulePath: string, args: readonly string[]): WorkerProcess {
  return utilityProcess.fork(modulePath, [...args], {
    serviceName: 'Talking Quill Whisper',
    stdio: 'ignore',
  });
}

function defaultForceKill(pid: number): void {
  forceKillProcess(pid, 'SIGKILL');
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => {
    const timer = setTimeout(resolve, milliseconds);
    timer.unref();
  });
}
