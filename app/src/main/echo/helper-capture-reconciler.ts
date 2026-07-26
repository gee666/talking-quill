const CAPACITY_RETRY_MS = 10;

export interface HelperCaptureReconcilerPort {
  setSessionCapture(active: boolean): Promise<unknown>;
  resetSessionCapture(signal?: AbortSignal): Promise<unknown>;
}

export class HelperCaptureReconciler {
  readonly #helper: HelperCaptureReconcilerPort;
  readonly #onCaptureOff: () => void;
  #generation = 0;
  #desired = false;
  #applied: boolean | null = false;
  #revision = 0;
  #running = false;
  #tail: Promise<void> = Promise.resolve();
  #captureOffGuaranteed = true;
  #disposed = false;
  readonly #shutdown = new AbortController();

  constructor(helper: HelperCaptureReconcilerPort, onCaptureOff: () => void) {
    this.#helper = helper;
    this.#onCaptureOff = onCaptureOff;
  }

  get captureOffGuaranteed(): boolean {
    return this.#captureOffGuaranteed;
  }

  beginGeneration(): number {
    this.#generation += 1;
    this.#revision += 1;
    return this.#generation;
  }

  markNativeCaptureArmed(): void {
    this.#revision += 1;
    this.#applied = null;
    this.#captureOffGuaranteed = false;
  }

  markAppliedUnknown(): void {
    this.#revision += 1;
    this.#applied = null;
  }

  beginShutdown(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    this.#desired = false;
    this.#revision += 1;
    this.#shutdown.abort(new DOMException('Capture reconciliation stopped', 'AbortError'));
  }

  requestBestEffort(active: boolean, generation: number): void {
    void this.request(active, generation).catch(() => undefined);
  }

  request(active: boolean, generation: number): Promise<void> {
    if (generation !== this.#generation || (this.#disposed && active)) {
      return Promise.resolve();
    }

    if (this.#desired !== active) {
      this.#desired = active;
      this.#revision += 1;
    }
    if (active) this.#captureOffGuaranteed = false;
    if (this.#running) return this.#tail;
    if (this.#applied === this.#desired) return Promise.resolve();
    this.#running = true;
    const reconcile = async () => {
      try {
        while ((!this.#disposed || !this.#desired) && this.#applied !== this.#desired) {
          const desired = this.#desired;
          const revision = this.#revision;
          try {
            await this.#helper.setSessionCapture(desired);
            if (revision !== this.#revision) {
              // Native activation may have re-armed capture while this acknowledgement was in
              // flight. Reconcile the newer revision instead of trusting the stale response.
              this.#applied = null;
              continue;
            }
            this.#applied = desired;
            if (!desired) this.#confirmCaptureOff();
          } catch (error: unknown) {
            this.#applied = null;
            if (revision !== this.#revision) continue;
            if (desired) throw normalizeCaptureError(error, 'Helper capture could not start');
            if (this.#disposed || helperSupervisionOwnsRecovery(error)) {
              this.#captureOffGuaranteed = false;
              throw normalizeCaptureError(error, 'Helper capture could not be disabled');
            }
            if (hasErrorCode(error, 'request-capacity')) {
              await delay(CAPACITY_RETRY_MS);
              continue;
            }
            try {
              await this.#helper.resetSessionCapture(this.#shutdown.signal);
              if (revision !== this.#revision) {
                this.#applied = null;
                continue;
              }
              this.#applied = false;
              this.#confirmCaptureOff();
            } catch (resetError: unknown) {
              if (revision !== this.#revision) {
                this.#applied = null;
                continue;
              }
              this.#captureOffGuaranteed = false;
              throw normalizeCaptureError(resetError, 'Helper capture could not be disabled');
            }
          }
        }
      } finally {
        this.#running = false;
      }
    };
    this.#tail = this.#tail.then(reconcile, reconcile);
    return this.#tail;
  }

  #confirmCaptureOff(): void {
    this.#captureOffGuaranteed = true;
    this.#onCaptureOff();
  }
}

function helperSupervisionOwnsRecovery(error: unknown): boolean {
  if (typeof error !== 'object' || error === null || !('code' in error)) return false;
  return (
    error.code === 'not-running' ||
    error.code === 'request-timeout' ||
    error.code === 'transport-error'
  );
}

function hasErrorCode(error: unknown, code: string): boolean {
  return typeof error === 'object' && error !== null && 'code' in error && error.code === code;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, milliseconds));
}

function normalizeCaptureError(error: unknown, fallback: string): Error {
  return error instanceof Error ? error : new Error(fallback);
}
