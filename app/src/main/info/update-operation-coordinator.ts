import { PublicAppError } from '../security/public-error';

export interface UpdateOperationOwner {
  readonly webContentsId: number;
  readonly onDestroyed: (listener: () => void) => () => void;
}

/** Isolated from provider operations so cancellation and quotas cannot cross domains. */
export class UpdateOperationCoordinator {
  readonly #operations = new Map<string, AbortController>();
  #disposed = false;

  async run<Result>(
    owner: UpdateOperationOwner,
    operationId: string,
    operation: (signal: AbortSignal) => Promise<Result>,
  ): Promise<Result> {
    if (this.#disposed) throw unavailable();
    const key = `${String(owner.webContentsId)}:${operationId}`;
    if (this.#operations.has(key)) {
      throw new PublicAppError({
        code: 'BAD_REQUEST',
        message: 'The update operation is invalid.',
      });
    }
    const controller = new AbortController();
    this.#operations.set(key, controller);
    const remove = owner.onDestroyed(() => controller.abort());
    try {
      return await operation(controller.signal);
    } finally {
      remove();
      this.#operations.delete(key);
    }
  }

  cancel(ownerId: number, operationId: string): boolean {
    const operation = this.#operations.get(`${String(ownerId)}:${operationId}`);
    if (operation === undefined) return false;
    operation.abort();
    return true;
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    for (const operation of this.#operations.values()) operation.abort();
    this.#operations.clear();
  }
}

function unavailable(): PublicAppError {
  return new PublicAppError({ code: 'UNAVAILABLE', message: 'Update checks are shutting down.' });
}
