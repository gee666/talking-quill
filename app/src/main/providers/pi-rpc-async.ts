import { ProviderError } from './errors';

export function asProviderError(
  error: unknown,
  fallback: ConstructorParameters<typeof ProviderError>[0],
): ProviderError {
  return error instanceof ProviderError ? error : new ProviderError(fallback);
}

export function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => {
    const timer = setTimeout(resolveDelay, milliseconds);
    timer.unref();
  });
}

export function deferred<Result>(): {
  readonly promise: Promise<Result>;
  readonly resolve: (result: Result) => void;
  readonly reject: (error: Error) => void;
} {
  let resolve!: (result: Result) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<Result>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return Object.freeze({ promise, resolve, reject });
}
