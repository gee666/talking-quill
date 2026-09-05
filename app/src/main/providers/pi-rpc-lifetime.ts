import { ProviderError } from './errors';

/** Owns the operation deadline and lifetime/prompt abort subscriptions. */
export class PiRpcLifetime {
  readonly #abort: () => void;
  #timeout: NodeJS.Timeout | null;
  #readyTimer: NodeJS.Timeout | null = null;
  #promptSignal: AbortSignal | undefined;

  constructor(
    private readonly signal: AbortSignal | undefined,
    timeoutMs: number,
    fail: (error: ProviderError) => void,
  ) {
    this.#abort = () => fail(new ProviderError('CANCELLED'));
    this.#timeout = setTimeout(() => fail(new ProviderError('TIMEOUT')), timeoutMs);
    this.#timeout.unref();
  }

  listen(): void {
    this.signal?.addEventListener('abort', this.#abort, { once: true });
  }

  listenPrompt(signal: AbortSignal | undefined): void {
    this.#promptSignal = signal;
    signal?.addEventListener('abort', this.#promptAbort, { once: true });
  }

  // A distinct callback preserves both subscriptions when the signals are the same object.
  readonly #promptAbort = (): void => this.#abort();

  deferReady(ready: () => void): void {
    this.#readyTimer = setTimeout(() => {
      this.#readyTimer = null;
      ready();
    }, 0);
  }

  clear(): void {
    if (this.#readyTimer !== null) {
      clearTimeout(this.#readyTimer);
      this.#readyTimer = null;
    }
    if (this.#timeout !== null) {
      clearTimeout(this.#timeout);
      this.#timeout = null;
    }
    this.signal?.removeEventListener('abort', this.#abort);
    this.#promptSignal?.removeEventListener('abort', this.#promptAbort);
    this.#promptSignal = undefined;
  }
}
