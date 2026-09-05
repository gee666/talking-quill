import {
  spawn,
  type ChildProcessWithoutNullStreams,
  type SpawnOptionsWithoutStdio,
} from 'node:child_process';

export type { ChildProcessWithoutNullStreams, SpawnOptionsWithoutStdio } from 'node:child_process';

export function defaultSpawnHelper(
  executablePath: string,
  options: SpawnOptionsWithoutStdio,
): ChildProcessWithoutNullStreams {
  return spawn(executablePath, [], {
    ...options,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
}

interface CloseWaiter {
  readonly promise: Promise<void>;
  readonly cancel: () => void;
}

export function waitForClose(child: ChildProcessWithoutNullStreams): CloseWaiter {
  let settled = false;
  let resolveClose: () => void = () => undefined;
  const onClose = (): void => {
    settled = true;
    resolveClose();
  };
  const promise = new Promise<void>((resolve) => {
    resolveClose = resolve;
    child.once('close', onClose);
  });
  return {
    promise,
    cancel: () => {
      if (!settled) child.removeListener('close', onClose);
    },
  };
}

export async function waitForOutcomeWithin(
  operation: Promise<void>,
  milliseconds: number,
): Promise<
  | { readonly completed: false; readonly error: null }
  | { readonly completed: true; readonly error: unknown }
> {
  const outcome = await waitForValueWithin(
    operation.then(
      () => ({ error: null }),
      (error: unknown) => ({ error }),
    ),
    milliseconds,
  );
  return outcome.completed
    ? { completed: true, error: outcome.value.error }
    : { completed: false, error: null };
}

export async function waitForValueWithin<Value>(
  operation: Promise<Value>,
  milliseconds: number,
): Promise<{ readonly completed: true; readonly value: Value } | { readonly completed: false }> {
  let resolveTimeout: (value: { readonly completed: false }) => void = () => undefined;
  const timeout = new Promise<{ readonly completed: false }>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(() => resolveTimeout({ completed: false }), milliseconds);
  timer.unref();
  try {
    return await Promise.race([
      operation.then((value) => ({ completed: true as const, value })),
      timeout,
    ]);
  } finally {
    clearTimeout(timer);
  }
}

export async function waitForCloseWithin(
  close: Promise<void>,
  milliseconds: number,
): Promise<boolean> {
  return (await waitForValueWithin(close, milliseconds)).completed;
}
