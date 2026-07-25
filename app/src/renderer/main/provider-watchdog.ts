const RENDERER_PROVIDER_WATCHDOG_MS = 125_000;

export function rendererWatchdog<Result>(
  operation: Promise<Result>,
  cancel: () => Promise<unknown> = () => Promise.resolve(),
): Promise<Result> {
  return new Promise<Result>((resolveOperation, rejectOperation) => {
    let settled = false;
    const finish = (error: unknown, result?: Result): void => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      if (error === null) resolveOperation(result as Result);
      else
        rejectOperation(
          error instanceof Error
            ? error
            : Object.assign(
                new Error('Provider operation failed'),
                typeof error === 'object' ? error : {},
              ),
        );
    };
    const timer = window.setTimeout(() => {
      const error = Object.assign(new Error('Renderer provider watchdog expired'), {
        code: 'TIMEOUT',
      });
      finish(error);
      void cancel().catch(() => undefined);
    }, RENDERER_PROVIDER_WATCHDOG_MS);
    void operation.then(
      (result) => finish(null, result),
      (error: unknown) => finish(error),
    );
  });
}
