import { utilityProcess } from 'electron';
import { join } from 'node:path';
import { kill as forceKillProcess } from 'node:process';
import { WhisperClientError } from './errors';
import type {
  WorkerProcess,
  WhisperWorkerSpawner,
  WhisperWorkerSupervisorOptions,
  TerminationKind,
  TerminationIntent,
} from './whisper-worker-contracts';
import { WhisperWorkerTermination } from './whisper-worker-termination';

const MAX_AUTOMATIC_RESTARTS = 5;
const MAX_RESTART_DELAY_MS = 2_000;
const HEALTH_TIMEOUT_MS = 5_000;
const STABILITY_RESET_MS = 30_000;

/** Owns worker generations, health handshakes, and bounded automatic restarts. */
export class WhisperWorkerProcess {
  readonly #cacheDirectory: string;
  readonly #workerPath: string;
  readonly #spawn: WhisperWorkerSpawner;
  readonly #onMessage: (generation: number, message: unknown) => void;
  readonly #onExit: (
    generation: number,
    termination: TerminationIntent | null,
    fallbackError: WhisperClientError,
  ) => void;
  readonly termination: WhisperWorkerTermination;
  process: WorkerProcess | null = null;
  generation = 0;
  lastExitedGeneration = 0;
  generationExit: Promise<void> | null = null;
  closing = false;
  #healthyGeneration = 0;
  #resolveGenerationExit: (() => void) | null = null;
  #restartAttempts = 0;
  #restartTimer: ReturnType<typeof setTimeout> | null = null;
  #healthTimer: ReturnType<typeof setTimeout> | null = null;
  #stabilityTimer: ReturnType<typeof setTimeout> | null = null;

  constructor(
    options: WhisperWorkerSupervisorOptions,
    onMessage: (generation: number, message: unknown) => void,
    onExit: (
      generation: number,
      termination: TerminationIntent | null,
      fallbackError: WhisperClientError,
    ) => void,
  ) {
    this.#cacheDirectory = options.cacheDirectory;
    this.#workerPath =
      options.workerPath ?? join(__dirname, '..', 'workers', 'whisper-bootstrap.cjs');
    this.#spawn = options.spawn ?? defaultSpawner;
    this.#onMessage = onMessage;
    this.#onExit = onExit;
    this.termination = new WhisperWorkerTermination(
      options.forceKill ?? defaultForceKill,
      () => this.process,
      (generation, confirmed) => this.#handleExit(generation, confirmed),
    );
  }

  beginTermination(
    generation: number,
    kind: TerminationKind,
    error: WhisperClientError,
    restart: boolean,
    cancelledRequestIds: ReadonlySet<string> | null = null,
  ): Promise<void> {
    if (generation !== this.generation || this.process === null) return Promise.resolve();
    return this.termination.beginTermination(
      generation,
      this.process,
      this.generationExit ?? Promise.resolve(),
      kind,
      error,
      restart,
      cancelledRequestIds,
    );
  }

  ensureProcess(): WorkerProcess {
    this.termination.retryRetiredProcessCleanup();
    if (this.termination.current !== null) {
      throw new WhisperClientError('WORKER_CRASHED', 'Whisper worker is still terminating.');
    }
    if (this.process !== null) return this.process;
    this.generation += 1;
    const generation = this.generation;
    const process = this.#spawn(this.#workerPath, [`--model-cache=${this.#cacheDirectory}`]);
    this.generationExit = new Promise<void>((resolve) => {
      this.#resolveGenerationExit = resolve;
    });
    process.on('message', (message) => this.#onMessage(generation, message));
    process.on('exit', () => this.#handleExit(generation));
    this.process = process;
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

  markHealthy(generation: number): void {
    if (generation !== this.generation || this.termination.current?.generation === generation)
      return;
    this.#healthyGeneration = generation;
    if (this.#healthTimer !== null) clearTimeout(this.#healthTimer);
    this.#healthTimer = null;
    if (this.#stabilityTimer !== null) clearTimeout(this.#stabilityTimer);
    this.#stabilityTimer = setTimeout(() => {
      if (this.#healthyGeneration === generation && this.process !== null) {
        this.#restartAttempts = 0;
      }
    }, STABILITY_RESET_MS);
    this.#stabilityTimer.unref();
  }

  #handleExit(generation: number, confirmed = true): void {
    if (confirmed && this.termination.confirmRetiredExit(generation)) return;
    if (generation !== this.generation || this.lastExitedGeneration === generation) return;
    this.lastExitedGeneration = generation;
    const termination =
      this.termination.current?.generation === generation ? this.termination.current : null;
    this.#clearGenerationTimers(termination);
    this.process = null;
    this.#healthyGeneration = 0;
    this.termination.current = null;
    this.#resolveGenerationExit?.();
    this.#resolveGenerationExit = null;
    this.generationExit = null;
    const fallbackError = this.closing
      ? new WhisperClientError('CANCELLED', 'Whisper worker closed.')
      : new WhisperClientError('WORKER_CRASHED', 'Whisper worker exited unexpectedly.');
    this.#onExit(generation, termination, fallbackError);
    if (confirmed) this.termination.releaseGenerationLeases(generation);
    if (!this.closing && (termination === null || termination.restart)) this.scheduleRestart();
  }

  scheduleRestart(): void {
    if (
      this.#restartTimer !== null ||
      this.closing ||
      this.#restartAttempts >= MAX_AUTOMATIC_RESTARTS
    ) {
      return;
    }
    const delayMs = Math.min(MAX_RESTART_DELAY_MS, 100 * 2 ** this.#restartAttempts);
    this.#restartAttempts += 1;
    this.#restartTimer = setTimeout(() => {
      this.#restartTimer = null;
      if (!this.closing && this.process === null && this.termination.current === null) {
        try {
          this.ensureProcess();
        } catch {
          this.scheduleRestart();
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
    this.termination.clearTimers(termination);
  }

  clearSupervisionTimers(): void {
    for (const timer of [this.#restartTimer, this.#healthTimer, this.#stabilityTimer]) {
      if (timer !== null) clearTimeout(timer);
    }
    this.#restartTimer = null;
    this.#healthTimer = null;
    this.#stabilityTimer = null;
  }
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
