import { ProviderError } from './errors';

export interface PiOperationPermit {
  release(): void;
}

export interface PiSpeculativeOperationPermit extends PiOperationPermit {
  /** Aborted when foreground work supersedes this lease before prompt commitment. */
  readonly revocationSignal: AbortSignal;
  /** Atomically makes the lease non-revocable immediately before prompt commitment. */
  commit(): boolean;
  /** Permanently prevents another Pi spawn when child cleanup could not be confirmed. */
  fail(error: ProviderError): void;
}

interface ActiveSpeculation {
  committed: boolean;
  readonly controller: AbortController;
}

/** Serializes every Pi child while allowing foreground work to revoke unused speculation. */
export class PiOperationScheduler {
  #queue: Promise<void> = Promise.resolve();
  #foregroundWaiters = 0;
  #activeSpeculation: ActiveSpeculation | null = null;
  #failure: ProviderError | null = null;
  #latestSpeculation = 0;

  async acquireForeground(signal: AbortSignal): Promise<PiOperationPermit> {
    this.#foregroundWaiters += 1;
    this.#revokeUncommittedSpeculation();
    try {
      return await this.#acquire(signal);
    } finally {
      this.#foregroundWaiters -= 1;
    }
  }

  async acquireSpeculative(signal: AbortSignal): Promise<PiSpeculativeOperationPermit> {
    // A newer speculative configuration supersedes an older unused process as well.
    const generation = ++this.#latestSpeculation;
    this.#revokeUncommittedSpeculation();
    const queuePermit = await this.#acquire(signal);
    if (signal.aborted) {
      queuePermit.release();
      throw new ProviderError('CANCELLED');
    }
    if (generation !== this.#latestSpeculation) {
      queuePermit.release();
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
    if (this.#foregroundWaiters > 0) {
      queuePermit.release();
      throw new ProviderError('UNAVAILABLE', { fallbackEligible: true });
    }
    const active: ActiveSpeculation = {
      committed: false,
      controller: new AbortController(),
    };
    this.#activeSpeculation = active;
    let released = false;
    const permit: PiSpeculativeOperationPermit = {
      revocationSignal: active.controller.signal,
      commit: () => {
        if (released || active.controller.signal.aborted) return false;
        active.committed = true;
        return true;
      },
      release: () => {
        if (released) return;
        released = true;
        if (this.#activeSpeculation === active) this.#activeSpeculation = null;
        queuePermit.release();
      },
      fail: (error: ProviderError) => {
        this.#failure ??= error;
        if (released) return;
        released = true;
        if (this.#activeSpeculation === active) this.#activeSpeculation = null;
        queuePermit.release();
      },
    };
    return Object.freeze(permit);
  }

  async #acquire(signal: AbortSignal): Promise<PiOperationPermit> {
    this.#assertAvailable();
    if (signal.aborted) throw new ProviderError('CANCELLED');
    const previous = this.#queue;
    let resolveTurn!: () => void;
    const turn = new Promise<void>((resolve) => {
      resolveTurn = resolve;
    });
    this.#queue = previous.catch(() => undefined).then(() => turn);
    let released = false;
    const release = (): void => {
      if (released) return;
      released = true;
      resolveTurn();
    };
    try {
      await waitForAbort(previous, signal);
      this.#assertAvailable();
      return Object.freeze({ release });
    } catch (error: unknown) {
      release();
      throw error;
    }
  }

  #assertAvailable(): void {
    if (this.#failure !== null) throw this.#failure;
  }

  #revokeUncommittedSpeculation(): void {
    const active = this.#activeSpeculation;
    if (active !== null && !active.committed && !active.controller.signal.aborted) {
      active.controller.abort();
    }
  }
}

function waitForAbort<Result>(operation: Promise<Result>, signal: AbortSignal): Promise<Result> {
  if (signal.aborted) return Promise.reject(new ProviderError('CANCELLED'));
  return new Promise<Result>((resolve, reject) => {
    const abort = (): void => reject(new ProviderError('CANCELLED'));
    signal.addEventListener('abort', abort, { once: true });
    void operation.then(
      (result) => {
        signal.removeEventListener('abort', abort);
        resolve(result);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof Error ? error : new ProviderError('UNAVAILABLE'));
      },
    );
  });
}
