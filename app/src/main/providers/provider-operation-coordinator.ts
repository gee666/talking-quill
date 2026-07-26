import { PublicAppError } from '../security/public-error';

const MAX_OPERATIONS_PER_RENDERER = 8;
const PROVIDER_IPC_HARD_TIMEOUT_MS = 120_000;

export interface ProviderOperationOwner {
  readonly webContentsId: number;
  readonly onDestroyed: (listener: () => void) => () => void;
}

export class ProviderOperationCoordinator {
  readonly #operations = new Map<
    string,
    { readonly ownerId: number; readonly controller: AbortController }
  >();
  #disposed = false;

  async run<Result>(
    owner: ProviderOperationOwner,
    operationId: string,
    operation: (signal: AbortSignal) => Promise<Result>,
  ): Promise<Result> {
    if (this.#disposed) throw unavailable();
    const key = operationKey(owner.webContentsId, operationId);
    if (this.#operations.has(key)) throw badRequest();
    const ownedCount = [...this.#operations.values()].filter(
      (candidate) => candidate.ownerId === owner.webContentsId,
    ).length;
    if (ownedCount >= MAX_OPERATIONS_PER_RENDERER) throw badRequest();

    const controller = new AbortController();
    this.#operations.set(key, { ownerId: owner.webContentsId, controller });
    const removeDestroyedListener = owner.onDestroyed(() => controller.abort());
    let timer: ReturnType<typeof setTimeout> | undefined;
    let cleaned = false;
    const cleanup = (): void => {
      if (cleaned) return;
      cleaned = true;
      if (timer !== undefined) clearTimeout(timer);
      removeDestroyedListener();
      this.#operations.delete(key);
    };
    let underlying: Promise<Result>;
    try {
      underlying = Promise.resolve(operation(controller.signal));
    } catch (error: unknown) {
      underlying = Promise.reject(error instanceof Error ? error : unavailable());
    }
    void underlying.then(cleanup, cleanup);
    const hardDeadline = new Promise<never>((_resolve, reject) => {
      timer = setTimeout(() => {
        controller.abort();
        cleanup();
        reject(timeout());
      }, PROVIDER_IPC_HARD_TIMEOUT_MS);
    });
    return await Promise.race([underlying, hardDeadline]);
  }

  cancel(ownerId: number, operationId: string): boolean {
    const operation = this.#operations.get(operationKey(ownerId, operationId));
    if (operation === undefined) return false;
    operation.controller.abort();
    return true;
  }

  cancelOwner(ownerId: number): void {
    for (const operation of this.#operations.values()) {
      if (operation.ownerId === ownerId) operation.controller.abort();
    }
  }

  dispose(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    for (const operation of this.#operations.values()) operation.controller.abort();
    this.#operations.clear();
  }
}

function operationKey(ownerId: number, operationId: string): string {
  return `${String(ownerId)}:${operationId}`;
}

function badRequest(): PublicAppError {
  return new PublicAppError({ code: 'BAD_REQUEST', message: 'The provider operation is invalid.' });
}

function timeout(): PublicAppError {
  return new PublicAppError({ code: 'TIMEOUT', message: 'The provider operation timed out.' });
}

function unavailable(): PublicAppError {
  return new PublicAppError({
    code: 'UNAVAILABLE',
    message: 'Provider operations are shutting down.',
  });
}
