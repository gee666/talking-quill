import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import { ModelManagerError } from './errors';

export interface ModelAccessLease {
  release(): void;
}

type AccessMode = 'use' | 'mutation';

interface Waiter {
  readonly mode: AccessMode;
  readonly resolve: (lease: ModelAccessLease) => void;
  readonly reject: (error: Error) => void;
  readonly signal?: AbortSignal;
  readonly onAbort?: () => void;
}

interface ModelLockState {
  readers: number;
  writer: boolean;
  readonly queue: Waiter[];
}

/**
 * A writer-preferring in-process lease. Model use leases cover verification,
 * pipeline load, and inference. Mutation leases cover unload and filesystem
 * publication/removal, so a new use cannot enter between those steps.
 */
export class ModelAccessCoordinator {
  readonly #states = new Map<WhisperModelId, ModelLockState>();

  acquireUse(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelAccessLease> {
    return this.#acquire(modelId, 'use', signal);
  }

  acquireMutation(modelId: WhisperModelId, signal?: AbortSignal): Promise<ModelAccessLease> {
    return this.#acquire(modelId, 'mutation', signal);
  }

  tryAcquireMutation(modelId: WhisperModelId): ModelAccessLease | null {
    const state = this.#states.get(modelId) ?? { readers: 0, writer: false, queue: [] };
    if (state.readers > 0 || state.writer || state.queue.length > 0) return null;
    state.writer = true;
    this.#states.set(modelId, state);
    let released = false;
    return {
      release: () => {
        if (released) return;
        released = true;
        state.writer = false;
        this.#drain(modelId, state);
      },
    };
  }

  async withMutation<Value>(
    modelId: WhisperModelId,
    operation: () => Promise<Value>,
    signal?: AbortSignal,
  ): Promise<Value> {
    const lease = await this.acquireMutation(modelId, signal);
    try {
      return await operation();
    } finally {
      lease.release();
    }
  }

  #acquire(
    modelId: WhisperModelId,
    mode: AccessMode,
    signal?: AbortSignal,
  ): Promise<ModelAccessLease> {
    if (signal?.aborted === true) {
      return Promise.reject(new ModelManagerError('CANCELLED', 'Model access was cancelled.'));
    }
    const state = this.#states.get(modelId) ?? { readers: 0, writer: false, queue: [] };
    this.#states.set(modelId, state);
    return new Promise((resolve, reject) => {
      const holder: { waiter: Waiter | null } = { waiter: null };
      const onAbort = () => {
        const waiter = holder.waiter;
        if (waiter === null) return;
        const index = state.queue.indexOf(waiter);
        if (index < 0) return;
        state.queue.splice(index, 1);
        reject(new ModelManagerError('CANCELLED', 'Model access was cancelled.'));
        this.#drain(modelId, state);
      };
      const waiter: Waiter = {
        mode,
        resolve,
        reject,
        ...(signal === undefined ? {} : { signal }),
        onAbort,
      };
      holder.waiter = waiter;
      signal?.addEventListener('abort', onAbort, { once: true });
      state.queue.push(waiter);
      this.#drain(modelId, state);
    });
  }

  #drain(modelId: WhisperModelId, state: ModelLockState): void {
    if (state.writer) return;
    const first = state.queue[0];
    if (first === undefined) {
      if (state.readers === 0) this.#states.delete(modelId);
      return;
    }
    if (first.mode === 'mutation') {
      if (state.readers > 0) return;
      state.queue.shift();
      state.writer = true;
      this.#grant(modelId, state, first, 'mutation');
      return;
    }
    while (state.queue[0]?.mode === 'use') {
      const reader = state.queue.shift();
      if (reader === undefined) break;
      state.readers += 1;
      this.#grant(modelId, state, reader, 'use');
    }
  }

  #grant(modelId: WhisperModelId, state: ModelLockState, waiter: Waiter, mode: AccessMode): void {
    waiter.signal?.removeEventListener('abort', waiter.onAbort ?? (() => undefined));
    let released = false;
    waiter.resolve({
      release: () => {
        if (released) return;
        released = true;
        if (mode === 'mutation') state.writer = false;
        else state.readers -= 1;
        this.#drain(modelId, state);
      },
    });
  }
}
