export interface HelperWakeEventSource {
  on(event: 'resume' | 'unlock-screen', listener: () => void): unknown;
  removeListener(event: 'resume' | 'unlock-screen', listener: () => void): unknown;
}

export interface HelperWakeRevalidatorOptions {
  readonly source: HelperWakeEventSource;
  readonly isSafeToRevalidate: () => boolean;
  readonly recycle: () => Promise<unknown>;
}

/** Replaces an idle helper after Windows may have invalidated its native keyboard hook. */
export function installHelperWakeRevalidator(options: HelperWakeRevalidatorOptions): () => void {
  const safetyRetryMs = 1_000;
  let disposed = false;
  let pendingWake = false;
  let safetyTimer: NodeJS.Timeout | null = null;
  let inFlight: Promise<unknown> | null = null;

  const scheduleSafetyRetry = (): void => {
    if (disposed || !pendingWake || safetyTimer !== null) return;
    safetyTimer = setTimeout(() => {
      safetyTimer = null;
      pumpRecovery();
    }, safetyRetryMs);
    safetyTimer.unref();
  };
  const pumpRecovery = (): void => {
    if (disposed || !pendingWake || inFlight !== null) return;
    if (!options.isSafeToRevalidate()) {
      scheduleSafetyRetry();
      return;
    }
    if (safetyTimer !== null) clearTimeout(safetyTimer);
    safetyTimer = null;
    pendingWake = false;
    const operation = Promise.resolve().then(() => {
      if (disposed) return;
      if (!options.isSafeToRevalidate()) {
        pendingWake = true;
        scheduleSafetyRetry();
        return;
      }
      return options.recycle();
    });
    inFlight = operation;
    void operation
      .catch(() => undefined)
      .finally(() => {
        if (inFlight !== operation) return;
        inFlight = null;
        pumpRecovery();
      });
  };
  const onWake = (): void => {
    if (disposed) return;
    pendingWake = true;
    pumpRecovery();
  };

  options.source.on('resume', onWake);
  options.source.on('unlock-screen', onWake);
  return () => {
    if (disposed) return;
    disposed = true;
    pendingWake = false;
    if (safetyTimer !== null) clearTimeout(safetyTimer);
    safetyTimer = null;
    options.source.removeListener('resume', onWake);
    options.source.removeListener('unlock-screen', onWake);
  };
}
