import type { WorkerProcess, TerminationKind, TerminationIntent } from './whisper-worker-contracts';
import type { WhisperClientError } from './errors';

const FORCE_KILL_AFTER_MS = 1_500;
const FORCE_KILL_RETRY_MS = 1_500;
const TERMINATION_DEADLINE_MS = FORCE_KILL_AFTER_MS + FORCE_KILL_RETRY_MS + 1_500;

/** Bounded process termination and leases retained for unconfirmed exits. */
export class WhisperWorkerTermination {
  readonly #forceKill: (pid: number) => void;
  readonly #activeProcess: () => WorkerProcess | null;
  readonly #onExit: (generation: number, confirmed: boolean) => void;
  readonly #sessionLeaseReleases = new Map<number, Set<() => void>>();
  readonly #retiredProcesses = new Map<number, WorkerProcess>();
  current: TerminationIntent | null = null;

  constructor(
    forceKill: (pid: number) => void,
    activeProcess: () => WorkerProcess | null,
    onExit: (generation: number, confirmed: boolean) => void,
  ) {
    this.#forceKill = forceKill;
    this.#activeProcess = activeProcess;
    this.#onExit = onExit;
  }

  confirmRetiredExit(generation: number): boolean {
    if (!this.#retiredProcesses.has(generation)) return false;
    this.#confirmRetiredGeneration(generation);
    return true;
  }

  clearTimers(termination: TerminationIntent | null): void {
    if (termination !== null) {
      if (termination.forceTimer !== null) clearTimeout(termination.forceTimer);
      if (termination.retryTimer !== null) clearTimeout(termination.retryTimer);
      if (termination.deadlineTimer !== null) clearTimeout(termination.deadlineTimer);
    }
  }

  beginTermination(
    generation: number,
    process: WorkerProcess,
    generationExited: Promise<void>,
    kind: TerminationKind,
    error: WhisperClientError,
    restart: boolean,
    cancelledRequestIds: ReadonlySet<string> | null = null,
  ): Promise<void> {
    const existing = this.current;
    if (existing?.generation === generation) {
      if (kind === 'close') {
        existing.kind = kind;
        existing.error = error;
        existing.restart = false;
      }
      return existing.settled;
    }
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
      terminationConfirmed: false,
      forceTimer: null,
      retryTimer: null,
      deadlineTimer: null,
    };
    this.current = termination;
    termination.terminationConfirmed = this.#tryKill(process);
    if (this.current !== termination || this.#activeProcess() !== process)
      return terminationSettled;
    termination.forceTimer = setTimeout(() => {
      if (this.current !== termination || this.#activeProcess() !== process) return;
      const pid = process.pid;
      if (pid === undefined) {
        termination.terminationConfirmed =
          this.#tryKill(process) || termination.terminationConfirmed;
      } else {
        try {
          this.#forceKill(pid);
        } catch {
          termination.terminationConfirmed =
            this.#tryKill(process) || termination.terminationConfirmed;
        }
      }
      termination.retryTimer = setTimeout(() => {
        if (this.current === termination && this.#activeProcess() === process) {
          termination.terminationConfirmed =
            this.#tryKill(process) || termination.terminationConfirmed;
        }
      }, FORCE_KILL_RETRY_MS);
      termination.retryTimer.unref();
    }, FORCE_KILL_AFTER_MS);
    termination.forceTimer.unref();
    termination.deadlineTimer = setTimeout(() => {
      if (this.current !== termination || this.#activeProcess() !== process) return;
      // Electron can terminate a utility process without emitting its exit event. Quarantine an
      // unconfirmed generation so replacement readers can proceed while its leases remain held.
      if (!termination.terminationConfirmed) this.#retiredProcesses.set(generation, process);
      this.#onExit(generation, termination.terminationConfirmed);
      resolveDeadline();
    }, TERMINATION_DEADLINE_MS);
    termination.deadlineTimer.unref();
    return terminationSettled;
  }

  releaseUseWhenSafe(generation: number, release: () => void): void {
    const terminatingCurrentGeneration =
      this.current?.generation === generation && this.#activeProcess() !== null;
    if (
      generation > 0 &&
      (terminatingCurrentGeneration || this.#retiredProcesses.has(generation))
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

  #tryKill(process: WorkerProcess): boolean {
    try {
      return process.kill();
    } catch {
      // The bounded termination deadline handles a process API that keeps failing.
      return false;
    }
  }

  retryRetiredProcessCleanup(): void {
    for (const [generation, process] of this.#retiredProcesses) {
      try {
        if (process.kill()) this.#confirmRetiredGeneration(generation);
      } catch {
        // Keep the quarantined handle so a later request or close can retry cleanup. Do not
        // force-kill by cached PID here because the exited process's PID may have been reused.
      }
    }
  }

  #confirmRetiredGeneration(generation: number): void {
    if (!this.#retiredProcesses.delete(generation)) return;
    this.releaseGenerationLeases(generation);
  }

  releaseGenerationLeases(generation: number): void {
    const releases = this.#sessionLeaseReleases.get(generation);
    if (releases === undefined) return;
    this.#sessionLeaseReleases.delete(generation);
    for (const release of releases) release();
  }
}
