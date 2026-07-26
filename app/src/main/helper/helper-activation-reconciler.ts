import {
  helperParamsSchemas,
  type ActivationBinding,
  type HelperParams,
} from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import { type HelperRpcSession } from './helper-rpc-channel';

const DEFAULT_REQUEST_TIMEOUT_MS = 3_000;

interface ActivationConfiguration {
  readonly enabled: boolean;
  readonly bindings: readonly ActivationBinding[];
}

interface ActivationReconcileOptions {
  readonly allowUnavailable: boolean;
  readonly timeoutMs: number;
  readonly timeoutReason: HelperReadinessReason;
}

interface HelperActivationReconcilerOptions {
  readonly getSession: () => HelperRpcSession | null;
  readonly isSessionAvailable: (session: HelperRpcSession) => boolean;
  readonly request: (
    session: HelperRpcSession,
    params: HelperParams<'activation.configure'>,
    timeoutMs: number,
    timeoutReason: HelperReadinessReason,
  ) => Promise<unknown>;
  readonly createNotRunningError: (message: string) => Error;
  readonly isNotRunningError: (error: unknown) => boolean;
}

const DEFAULT_RECONCILE_OPTIONS: ActivationReconcileOptions = {
  allowUnavailable: true,
  timeoutMs: DEFAULT_REQUEST_TIMEOUT_MS,
  timeoutReason: 'request-timeout',
};

/** Internal owner of retained activation intent and process-scoped reconciliation. */
export class HelperActivationReconciler {
  readonly #options: HelperActivationReconcilerOptions;
  #desired: ActivationConfiguration = Object.freeze({ enabled: false, bindings: [] });
  #blockedByHealth = true;
  #revision = 0;
  #applied: { readonly session: HelperRpcSession; readonly revision: number } | null = null;
  #intentTail: Promise<void> = Promise.resolve();
  #reconcileTail: Promise<void> = Promise.resolve();

  constructor(options: HelperActivationReconcilerOptions) {
    this.#options = options;
  }

  configure(
    enabled: boolean,
    bindings: readonly ActivationBinding[],
  ): Promise<ActivationConfiguration> {
    const configured = helperParamsSchemas['activation.configure'].parse({
      enabled,
      bindings: [...bindings],
    });
    const desired = Object.freeze({
      enabled: configured.enabled,
      bindings: Object.freeze(configured.bindings.map((binding) => Object.freeze(binding))),
    });
    const apply = async (): Promise<ActivationConfiguration> => {
      const previous = this.#desired;
      this.#desired = desired;
      this.#revision += 1;
      try {
        await this.reconcile();
        return desired;
      } catch (error: unknown) {
        this.#desired = previous;
        this.#revision += 1;
        await this.reconcile().catch(() => undefined);
        throw error;
      }
    };
    const operation = this.#intentTail.then(apply, apply);
    this.#intentTail = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  }

  setBlockedByHealth(blocked: boolean): void {
    if (this.#blockedByHealth === blocked) return;
    this.#blockedByHealth = blocked;
    this.#revision += 1;
  }

  prepareFreshSession(): void {
    this.#applied = null;
    this.setBlockedByHealth(true);
  }

  processUnavailable(session: HelperRpcSession): void {
    if (this.#applied?.session === session) this.#applied = null;
    this.setBlockedByHealth(true);
  }

  reconcile(
    allowUnavailable = DEFAULT_RECONCILE_OPTIONS.allowUnavailable,
    timeoutMs = DEFAULT_RECONCILE_OPTIONS.timeoutMs,
    timeoutReason = DEFAULT_RECONCILE_OPTIONS.timeoutReason,
  ): Promise<void> {
    return this.#enqueueReconcile(() =>
      this.#reconcile({ allowUnavailable, timeoutMs, timeoutReason }),
    );
  }

  reconcileSession(
    session: HelperRpcSession,
    allowUnavailable: boolean,
    timeoutMs = DEFAULT_RECONCILE_OPTIONS.timeoutMs,
    timeoutReason = DEFAULT_RECONCILE_OPTIONS.timeoutReason,
  ): Promise<void> {
    return this.#enqueueReconcile(() =>
      this.#reconcile({ allowUnavailable, timeoutMs, timeoutReason }, session),
    );
  }

  reconcileFreshHelper(
    session: HelperRpcSession,
    timeoutMs: number,
    timeoutReason: HelperReadinessReason,
  ): Promise<void> {
    return this.#enqueueReconcile(async () => {
      const options = { allowUnavailable: false, timeoutMs, timeoutReason };
      if (this.#options.getSession() !== session || !this.#options.isSessionAvailable(session)) {
        this.#applied = null;
        throw this.#options.createNotRunningError('Native helper activation changed process');
      }
      if (this.#blockedByHealth || !this.#desired.enabled) {
        this.#applied = { session, revision: this.#revision };
        return;
      }
      await this.#reconcile(options);
    });
  }

  #enqueueReconcile(operation: () => Promise<void>): Promise<void> {
    const queued = this.#reconcileTail.then(operation, operation);
    this.#reconcileTail = queued.catch(() => undefined);
    return queued;
  }

  async #reconcile(
    options: ActivationReconcileOptions,
    expectedSession?: HelperRpcSession,
  ): Promise<void> {
    for (;;) {
      const revision = this.#revision;
      const session = this.#options.getSession();
      if (expectedSession !== undefined && session !== expectedSession) {
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation changed process');
      }
      if (session !== null && this.#applied?.session === session) {
        if (this.#applied.revision === revision) return;
      }
      if (session === null || !this.#options.isSessionAvailable(session)) {
        this.#applied = null;
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation is unavailable');
      }
      const desired = this.#desired;
      try {
        await this.#options.request(
          session,
          {
            enabled: !this.#blockedByHealth && desired.enabled,
            bindings: [...desired.bindings],
          },
          options.timeoutMs,
          options.timeoutReason,
        );
      } catch (error: unknown) {
        if (options.allowUnavailable && this.#options.isNotRunningError(error)) {
          this.#applied = null;
          return;
        }
        throw error;
      }
      if (this.#options.getSession() !== session) {
        this.#applied = null;
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation changed process');
      }
      if (revision === this.#revision) {
        this.#applied = { session, revision };
        return;
      }
    }
  }
}
