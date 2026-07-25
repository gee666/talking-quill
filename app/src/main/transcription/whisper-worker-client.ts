import { utilityProcess } from 'electron';
import { randomUUID } from 'node:crypto';
import { kill as forceKillProcess } from 'node:process';
import { join } from 'node:path';
import { WHISPER_PROTOCOL_VERSION } from '../../shared/constants/whisper';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import {
  TranscriptionOptionsSchema,
  type ModelStatus,
  type TranscriptionOptions,
  type TranscriptionResult,
} from '../../shared/schemas/transcription';
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
const MODEL_CHECK_TIMEOUT_MS = 120_000;
const STABILITY_RESET_MS = 30_000;
const SHUTDOWN_GRACE_MS = 1_000;
const FORCE_KILL_AFTER_MS = 1_500;
const FORCE_KILL_RETRY_MS = 1_500;

interface WorkerProcess {
  readonly pid: number | undefined;
  postMessage(message: unknown): void;
  kill(): boolean;
  on(event: 'message', listener: (message: unknown) => void): this;
  on(event: 'exit', listener: (code: number) => void): this;
}

export type WhisperWorkerSpawner = (modulePath: string, args: readonly string[]) => WorkerProcess;

export interface AcquiredModelUse {
  readonly status: ModelStatus;
  release(): void;
}

export type ModelUseAcquirer = (
  modelId: WhisperModelId,
  signal?: AbortSignal,
) => Promise<AcquiredModelUse>;

interface PendingRequest {
  readonly generation: number;
  readonly resolve: (result: WhisperWorkerResult) => void;
  readonly reject: (error: Error) => void;
}

type TerminationKind = 'cancel' | 'close' | 'health' | 'protocol' | 'unavailable';

interface TerminationIntent {
  readonly generation: number;
  kind: TerminationKind;
  error: WhisperClientError;
  restart: boolean;
  readonly exited: Promise<void>;
  forceTimer: ReturnType<typeof setTimeout> | null;
  retryTimer: ReturnType<typeof setTimeout> | null;
}

export interface WhisperStreamingSession {
  readonly id: string;
  push(pcm: Float32Array, signal?: AbortSignal): Promise<void>;
  finish(signal?: AbortSignal): Promise<TranscriptionResult>;
  cancel(): Promise<void>;
}

export class WhisperWorkerClient {
  readonly #cacheDirectory: string;
  readonly #workerPath: string;
  readonly #spawn: WhisperWorkerSpawner;
  readonly #acquireModelUse: ModelUseAcquirer;
  readonly #forceKill: (pid: number) => void;
  readonly #pending = new Map<string, PendingRequest>();
  readonly #sessionLeaseReleases = new Map<number, Set<() => void>>();
  #process: WorkerProcess | null = null;
  #generation = 0;
  #lastExitedGeneration = 0;
  #healthyGeneration = 0;
  #generationExit: Promise<void> | null = null;
  #resolveGenerationExit: (() => void) | null = null;
  #termination: TerminationIntent | null = null;
  #closing = false;
  #closePromise: Promise<void> | null = null;
  #restartAttempts = 0;
  #restartTimer: ReturnType<typeof setTimeout> | null = null;
  #healthTimer: ReturnType<typeof setTimeout> | null = null;
  #stabilityTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(options: {
    readonly cacheDirectory: string;
    readonly acquireModelUse: ModelUseAcquirer;
    readonly workerPath?: string;
    readonly spawn?: WhisperWorkerSpawner;
    readonly forceKill?: (pid: number) => void;
  }) {
    this.#cacheDirectory = options.cacheDirectory;
    this.#workerPath =
      options.workerPath ?? join(__dirname, '..', 'workers', 'whisper-bootstrap.cjs');
    this.#spawn = options.spawn ?? defaultSpawner;
    this.#acquireModelUse = options.acquireModelUse;
    this.#forceKill = options.forceKill ?? defaultForceKill;
  }

  async transcribe(
    pcm: Float32Array,
    options: TranscriptionOptions,
    signal: AbortSignal,
  ): Promise<TranscriptionResult> {
    const validated = TranscriptionOptionsSchema.parse(options);
    const use = await this.#acquireReadyModel(validated.modelId, signal);
    try {
      const result = await this.#request(
        (requestId) => ({
          version: WHISPER_PROTOCOL_VERSION,
          requestId,
          type: 'transcribe',
          pcm: copyPcm(pcm),
          options: validated,
        }),
        signal,
      );
      if (result.type !== 'transcription') {
        throw new WhisperClientError('PROTOCOL_ERROR', 'Worker returned the wrong response.');
      }
      return result.value;
    } finally {
      use.release();
    }
  }

  async startSession(
    options: TranscriptionOptions,
    signal?: AbortSignal,
  ): Promise<WhisperStreamingSession> {
    const validated = TranscriptionOptionsSchema.parse(options);
    const use = await this.#acquireReadyModel(validated.modelId, signal);
    try {
      const sessionId = randomUUID();
      const opened = await this.#request(
        (requestId) => ({
          version: WHISPER_PROTOCOL_VERSION,
          requestId,
          type: 'session-open',
          sessionId,
          options: validated,
        }),
        signal,
      );
      assertAcknowledged(opened, 'session-open');
      const sessionGeneration = this.#generation;
      let closed = false;
      const releaseUse = once(() => use.release());
      this.#registerSessionLease(sessionGeneration, releaseUse);
      const closeSession = () => {
        if (closed) return false;
        closed = true;
        this.#unregisterSessionLease(sessionGeneration, releaseUse);
        return true;
      };
      return {
        id: sessionId,
        push: async (pcm, pushSignal) => {
          if (closed) throw new WhisperClientError('CANCELLED', 'Streaming session is closed.');
          if (this.#generation !== sessionGeneration || this.#process === null) {
            closeSession();
            releaseUse();
            throw new WhisperClientError('WORKER_CRASHED', 'Streaming worker generation changed.');
          }
          const pushed = await this.#request(
            (requestId) => ({
              version: WHISPER_PROTOCOL_VERSION,
              requestId,
              type: 'session-push',
              sessionId,
              pcm: copyPcm(pcm),
            }),
            pushSignal,
          );
          assertAcknowledged(pushed, 'session-push');
        },
        finish: async (finishSignal) => {
          if (!closeSession()) {
            throw new WhisperClientError('CANCELLED', 'Streaming session is closed.');
          }
          try {
            if (this.#generation !== sessionGeneration || this.#process === null) {
              throw new WhisperClientError(
                'WORKER_CRASHED',
                'Streaming worker generation changed.',
              );
            }
            const result = await this.#request(
              (requestId) => ({
                version: WHISPER_PROTOCOL_VERSION,
                requestId,
                type: 'session-finish',
                sessionId,
              }),
              finishSignal,
            );
            if (result.type !== 'transcription') {
              throw new WhisperClientError('PROTOCOL_ERROR', 'Worker returned the wrong response.');
            }
            return result.value;
          } finally {
            releaseUse();
          }
        },
        cancel: async () => {
          if (!closeSession()) return;
          try {
            await this.#beginTermination(
              sessionGeneration,
              'cancel',
              new WhisperClientError('CANCELLED', 'Streaming transcription was cancelled.'),
              false,
            );
          } finally {
            releaseUse();
          }
        },
      };
    } catch (error: unknown) {
      use.release();
      throw error;
    }
  }

  async checkWorkerModel(modelId: WhisperModelId, signal?: AbortSignal): Promise<'ready'> {
    const timeoutController = new AbortController();
    const timeout = setTimeout(
      () => timeoutController.abort('model validation timeout'),
      MODEL_CHECK_TIMEOUT_MS,
    );
    timeout.unref();
    try {
      const result = await this.#request(
        (requestId) => ({
          version: WHISPER_PROTOCOL_VERSION,
          requestId,
          type: 'model-check',
          modelId,
        }),
        signal === undefined
          ? timeoutController.signal
          : AbortSignal.any([signal, timeoutController.signal]),
      );
      if (result.type !== 'model-ready') {
        throw new WhisperClientError(
          'PROTOCOL_ERROR',
          'Worker model check returned the wrong response.',
        );
      }
      return 'ready';
    } catch (error: unknown) {
      if (timeoutController.signal.aborted && signal?.aborted !== true) {
        throw new WhisperClientError('WORKER_CRASHED', 'Offline model validation timed out.');
      }
      throw error;
    } finally {
      clearTimeout(timeout);
    }
  }

  async unload(modelId?: WhisperModelId): Promise<void> {
    await this.#waitForTermination();
    if (this.#process === null) return;
    const result = await this.#request((requestId) => ({
      version: WHISPER_PROTOCOL_VERSION,
      requestId,
      type: 'unload',
      ...(modelId === undefined ? {} : { modelId }),
    }));
    assertAcknowledged(result, 'unload');
  }

  async memoryPressure(): Promise<void> {
    await this.#waitForTermination();
    if (this.#process === null) return;
    const result = await this.#request((requestId) => ({
      version: WHISPER_PROTOCOL_VERSION,
      requestId,
      type: 'memory-pressure',
    }));
    assertAcknowledged(result, 'memory-pressure');
  }

  close(): Promise<void> {
    this.#closePromise ??= this.#closeInternal();
    return this.#closePromise;
  }

  async #closeInternal(): Promise<void> {
    this.#closing = true;
    this.#clearSupervisionTimers();
    const process = this.#process;
    const generation = this.#generation;
    const exited = this.#generationExit;
    if (process === null || exited === null) {
      this.#rejectPending(new WhisperClientError('CANCELLED', 'Whisper worker closed.'));
      return;
    }
    if (this.#termination !== null) {
      await this.#beginTermination(
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
    const shutdown = this.#request(
      (requestId) => ({ version: WHISPER_PROTOCOL_VERSION, requestId, type: 'shutdown' }),
      undefined,
      true,
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
      await this.#beginTermination(
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

  async #acquireReadyModel(
    modelId: WhisperModelId,
    signal?: AbortSignal,
  ): Promise<AcquiredModelUse> {
    const use = await this.#acquireModelUse(modelId, signal);
    if (use.status.state === 'ready') return use;
    use.release();
    if (use.status.state === 'corrupt') {
      throw new WhisperClientError('MODEL_CORRUPT', 'The selected local model is corrupt.');
    }
    throw new WhisperClientError('MODEL_MISSING', 'The selected local model is not installed.');
  }

  async #request(
    create: (requestId: string) => WhisperWorkerRequest,
    signal?: AbortSignal,
    allowClosing = false,
  ): Promise<WhisperWorkerResult> {
    await this.#waitForTermination();
    if (this.#closing && !allowClosing) {
      throw new WhisperClientError('CANCELLED', 'Whisper worker is closing.');
    }
    if (signal?.aborted === true) {
      throw new WhisperClientError('CANCELLED', 'Transcription was cancelled.');
    }
    let process: WorkerProcess;
    try {
      process = this.#ensureProcess();
    } catch (error: unknown) {
      throw error instanceof Error
        ? error
        : new WhisperClientError('WORKER_CRASHED', 'Whisper worker could not start.');
    }
    const generation = this.#generation;
    const requestId = randomUUID();
    const request = WhisperWorkerRequestSchema.parse(create(requestId));
    return new Promise((resolve, reject) => {
      const onAbort = () => {
        void this.#beginTermination(
          generation,
          'cancel',
          new WhisperClientError('CANCELLED', 'Transcription was cancelled.'),
          false,
        );
      };
      signal?.addEventListener('abort', onAbort, { once: true });
      this.#pending.set(requestId, {
        generation,
        resolve: (result) => {
          signal?.removeEventListener('abort', onAbort);
          resolve(result);
        },
        reject: (error) => {
          signal?.removeEventListener('abort', onAbort);
          reject(error);
        },
      });
      try {
        process.postMessage(request);
      } catch {
        void this.#beginTermination(
          generation,
          'unavailable',
          new WhisperClientError('WORKER_CRASHED', 'Whisper worker was unavailable.'),
          true,
        );
      }
    });
  }

  #ensureProcess(): WorkerProcess {
    if (this.#process !== null) return this.#process;
    if (this.#termination !== null) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker is still terminating.');
    }
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
        void this.#beginTermination(
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
      void this.#beginTermination(
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
    const error =
      termination?.error ??
      (this.#closing
        ? new WhisperClientError('CANCELLED', 'Whisper worker closed.')
        : new WhisperClientError('WORKER_CRASHED', 'Whisper worker exited unexpectedly.'));
    for (const [id, pending] of this.#pending) {
      if (pending.generation !== generation) continue;
      this.#pending.delete(id);
      pending.reject(error);
    }
    const sessionReleases = this.#sessionLeaseReleases.get(generation);
    if (sessionReleases !== undefined) {
      this.#sessionLeaseReleases.delete(generation);
      for (const release of sessionReleases) release();
    }
    if (!this.#closing && (termination === null || termination.restart)) this.#scheduleRestart();
  }

  #beginTermination(
    generation: number,
    kind: TerminationKind,
    error: WhisperClientError,
    restart: boolean,
  ): Promise<void> {
    if (generation !== this.#generation || this.#process === null) return Promise.resolve();
    const existing = this.#termination;
    if (existing?.generation === generation) {
      if (kind === 'close') {
        existing.kind = kind;
        existing.error = error;
        existing.restart = false;
      }
      return existing.exited;
    }
    const process = this.#process;
    const exited = this.#generationExit ?? Promise.resolve();
    const termination: TerminationIntent = {
      generation,
      kind,
      error,
      restart,
      exited,
      forceTimer: null,
      retryTimer: null,
    };
    this.#termination = termination;
    process.kill();
    if (this.#termination !== termination || this.#process !== process) return exited;
    termination.forceTimer = setTimeout(() => {
      if (this.#termination !== termination || this.#process !== process) return;
      const pid = process.pid;
      if (pid === undefined) process.kill();
      else {
        try {
          this.#forceKill(pid);
        } catch {
          process.kill();
        }
      }
      termination.retryTimer = setTimeout(() => {
        if (this.#termination === termination && this.#process === process) process.kill();
      }, FORCE_KILL_RETRY_MS);
      termination.retryTimer.unref();
    }, FORCE_KILL_AFTER_MS);
    termination.forceTimer.unref();
    return exited;
  }

  async #waitForTermination(): Promise<void> {
    const termination = this.#termination;
    if (termination !== null) await termination.exited;
  }

  #scheduleRestart(): void {
    if (
      this.#restartTimer !== null ||
      this.#closing ||
      this.#restartAttempts >= MAX_AUTOMATIC_RESTARTS
    ) {
      return;
    }
    const delayMs = Math.min(MAX_RESTART_DELAY_MS, 100 * 2 ** this.#restartAttempts);
    this.#restartAttempts += 1;
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      if (!this.#closing && this.#process === null && this.#termination === null) {
        try {
          this.#ensureProcess();
        } catch {
          this.#scheduleRestart();
        }
      }
    }, delayMs);
    this.#restartTimer.unref();
  }

  #registerSessionLease(generation: number, release: () => void): void {
    const releases = this.#sessionLeaseReleases.get(generation) ?? new Set<() => void>();
    releases.add(release);
    this.#sessionLeaseReleases.set(generation, releases);
  }

  #unregisterSessionLease(generation: number, release: () => void): void {
    const releases = this.#sessionLeaseReleases.get(generation);
    releases?.delete(release);
    if (releases?.size === 0) this.#sessionLeaseReleases.delete(generation);
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

function copyPcm(pcm: Float32Array): ArrayBuffer {
  const copy = new Float32Array(pcm.length);
  copy.set(pcm);
  return copy.buffer;
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

function once(operation: () => void): () => void {
  let called = false;
  return () => {
    if (called) return;
    called = true;
    operation();
  };
}
