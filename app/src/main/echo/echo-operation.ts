export function raceWithAbort<Value>(
  operation: Promise<Value>,
  signal: AbortSignal,
): Promise<Value> {
  if (signal.aborted) return Promise.reject(abortOperationError());
  return new Promise<Value>((resolve, reject) => {
    const abort = () => reject(abortOperationError());
    signal.addEventListener('abort', abort, { once: true });
    operation.then(
      (value) => {
        signal.removeEventListener('abort', abort);
        resolve(value);
      },
      (error: unknown) => {
        signal.removeEventListener('abort', abort);
        reject(error instanceof Error ? error : new Error('Echo operation failed'));
      },
    );
  });
}

export function normalizeOperationError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}

export function operationError(
  operation: () => Promise<unknown>,
  fallback: string,
): Promise<Error | null> {
  try {
    return operation().then(
      () => null,
      (error: unknown) => normalizeOperationError(error, fallback),
    );
  } catch (error: unknown) {
    return Promise.resolve(normalizeOperationError(error, fallback));
  }
}

export function abortOperationError(): Error {
  const error = new Error('Echo operation cancelled');
  error.name = 'AbortError';
  return error;
}

export function withSoftTimeout<Value>(
  operation: Promise<Value>,
  timeoutMs: number,
  fallback: Value,
): Promise<Value> {
  return new Promise((resolve) => {
    const timer = setTimeout(() => resolve(fallback), timeoutMs);
    timer.unref();
    operation.then(
      (value) => {
        clearTimeout(timer);
        resolve(value);
      },
      () => {
        clearTimeout(timer);
        resolve(fallback);
      },
    );
  });
}
