import {
  type ActivationBinding,
  type HelperParams,
  type HelperResult,
} from '../../shared/helper/protocol';
import { type HelperReadinessReason } from '../../shared/schemas/helper-readiness';
import {
  activationAcknowledgementMatches,
  createActivationConfiguration,
  type ActivationConfiguration,
} from './activation-configuration';
import { type HelperRpcSession } from './helper-rpc-channel';

const DEFAULT_REQUEST_TIMEOUT_MS = 3_000;

interface ActivationReconcileOptions {
  readonly allowUnavailable: boolean;
  readonly timeoutMs: number;
  readonly timeoutReason: HelperReadinessReason;
}

interface HelperActivationReconcilerOptions {
  readonly getSession: () => HelperRpcSession | null;
  readonly isSessionAvailable: (session: HelperRpcSession) => boolean;
  readonly isSessionCurrent: (session: HelperRpcSession) => boolean;
  readonly request: (
    session: HelperRpcSession,
    params: HelperParams<'activation.configure'>,
    timeoutMs: number,
    timeoutReason: HelperReadinessReason,
  ) => Promise<HelperResult<'activation.configure'>>;
  readonly createNotRunningError: (message: string) => Error;
  readonly createProtocolError: (message: string) => Error;
  readonly reportProtocolFault: (session: HelperRpcSession, error: Error) => void;
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
  #physicalObservationActive = false;
  #effective: { readonly session: HelperRpcSession; readonly enabled: boolean } | null = null;
  #revision = 0;
  #applied: { readonly session: HelperRpcSession; readonly revision: number } | null = null;
  #intentTail: Promise<void> = Promise.resolve();
  #reconcileTail: Promise<void> = Promise.resolve();

  constructor(options: HelperActivationReconcilerOptions) {
    this.#options = options;
  }

  get effectiveEnabled(): boolean | null {
    const effective = this.#effective;
    return effective !== null && effective.session === this.#options.getSession()
      ? effective.enabled
      : null;
  }

  configure(
    enabled: boolean,
    bindings: readonly ActivationBinding[],
  ): Promise<ActivationConfiguration> {
    const desired = createActivationConfiguration(enabled, bindings);
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
    this.#effective = null;
    this.setBlockedByHealth(true);
  }

  async beginPhysicalObservation(): Promise<void> {
    await this.#enqueueReconcile(async () => {
      const session = this.#options.getSession();
      if (session === null || !this.#options.isSessionAvailable(session)) {
        throw this.#options.createNotRunningError('Native physical observation is unavailable');
      }
      const requested = { enabled: false, bindings: [...this.#desired.bindings] };
      const revision = this.#revision;
      const effective = await this.#options.request(
        session,
        requested,
        DEFAULT_REQUEST_TIMEOUT_MS,
        'request-timeout',
      );
      if (!activationAcknowledgementMatches(requested, effective) || effective.enabled) {
        throw this.#options.createProtocolError(
          'Native helper did not enter passive physical observation',
        );
      }
      this.#effective = { session, enabled: false };
      this.#physicalObservationActive = true;
      this.#revision += 1;
      this.#applied =
        revision + 1 === this.#revision ? { session, revision: this.#revision } : null;
    });
  }

  endPhysicalObservation(): Promise<void> {
    return this.#enqueueReconcile(async () => {
      if (this.#physicalObservationActive) {
        this.#physicalObservationActive = false;
        this.#revision += 1;
      }
      await this.#reconcile({ ...DEFAULT_RECONCILE_OPTIONS, allowUnavailable: false });
    });
  }

  processUnavailable(session: HelperRpcSession): void {
    if (this.#applied?.session === session) this.#applied = null;
    if (this.#effective?.session === session) this.#effective = null;
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
    onAuthoritative: () => void,
  ): Promise<void> {
    return this.#enqueueReconcile(async () => {
      const options = { allowUnavailable: false, timeoutMs, timeoutReason };
      for (;;) {
        this.#assertFreshSession(session);
        const revision = this.#revision;
        const requested = {
          enabled: false,
          bindings: [...this.#desired.bindings],
        };
        const effective = await this.#options.request(
          session,
          requested,
          options.timeoutMs,
          options.timeoutReason,
        );
        this.#assertFreshSession(session);
        if (!activationAcknowledgementMatches(requested, effective) || effective.enabled) {
          const error = this.#options.createProtocolError(
            'Native helper did not confirm disabled-first activation reconciliation',
          );
          this.#options.reportProtocolFault(session, error);
          throw error;
        }
        this.#effective = { session, enabled: false };
        if (revision !== this.#revision) continue;
        if (this.#blockedByHealth || this.#physicalObservationActive || !this.#desired.enabled) {
          this.#applied = { session, revision };
          onAuthoritative();
          return;
        }
        await this.#reconcileFreshEnable(options, session);
        onAuthoritative();
        return;
      }
    });
  }

  #assertFreshSession(session: HelperRpcSession): void {
    if (this.#options.getSession() !== session || !this.#options.isSessionCurrent(session)) {
      this.#applied = null;
      if (this.#effective?.session === session) this.#effective = null;
      throw this.#options.createNotRunningError('Native helper activation changed process');
    }
  }

  async #reconcileFreshEnable(
    options: ActivationReconcileOptions,
    session: HelperRpcSession,
  ): Promise<void> {
    for (;;) {
      this.#assertFreshSession(session);
      const revision = this.#revision;
      const desired = this.#desired;
      if (this.#blockedByHealth || this.#physicalObservationActive || !desired.enabled) {
        this.#effective = { session, enabled: false };
        this.#applied = { session, revision };
        return;
      }
      const requested = { enabled: true, bindings: [...desired.bindings] };
      const effective = await this.#options.request(
        session,
        requested,
        options.timeoutMs,
        options.timeoutReason,
      );
      this.#assertFreshSession(session);
      if (!activationAcknowledgementMatches(requested, effective)) {
        const error = this.#options.createProtocolError(
          'Native helper returned a mismatched activation configuration',
        );
        this.#options.reportProtocolFault(session, error);
        throw error;
      }
      this.#effective = { session, enabled: effective.enabled };
      if (revision === this.#revision) {
        this.#applied = { session, revision };
        return;
      }
    }
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
        if (this.#effective?.session === expectedSession) this.#effective = null;
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation changed process');
      }
      if (session !== null && this.#applied?.session === session) {
        if (this.#applied.revision === revision) return;
      }
      if (session === null || !this.#options.isSessionAvailable(session)) {
        this.#applied = null;
        if (session === null || this.#effective?.session === session) this.#effective = null;
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation is unavailable');
      }
      const desired = this.#desired;
      const requested = {
        enabled: !this.#blockedByHealth && !this.#physicalObservationActive && desired.enabled,
        bindings: [...desired.bindings],
      };
      let effective: HelperResult<'activation.configure'>;
      try {
        effective = await this.#options.request(
          session,
          requested,
          options.timeoutMs,
          options.timeoutReason,
        );
      } catch (error: unknown) {
        if (options.allowUnavailable && this.#options.isNotRunningError(error)) {
          this.#applied = null;
          if (this.#effective?.session === session) this.#effective = null;
          return;
        }
        throw error;
      }
      if (this.#options.getSession() !== session) {
        this.#applied = null;
        if (this.#effective?.session === session) this.#effective = null;
        if (options.allowUnavailable) return;
        throw this.#options.createNotRunningError('Native helper activation changed process');
      }
      if (!activationAcknowledgementMatches(requested, effective)) {
        const error = this.#options.createProtocolError(
          'Native helper returned a mismatched activation configuration',
        );
        this.#options.reportProtocolFault(session, error);
        throw error;
      }
      this.#effective = { session, enabled: effective.enabled };
      if (revision === this.#revision) {
        this.#applied = { session, revision };
        return;
      }
    }
  }
}
