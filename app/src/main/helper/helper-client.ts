import { waitForClose, waitForCloseWithin } from './helper-process';
import { HelperClientError } from './helper-client-error';
import {
  HelperRuntimeObservabilitySchema,
  type ActivationBinding,
  type HelperActivationContext,
  type HelperFrontApp,
  type HelperMethod,
  type HelperNotification,
  type HelperParams,
  type HelperPasteResult,
  type HelperPermissions,
  type HelperPrepareMaintenanceParams,
  type HelperResult,
  type HelperRuntimeObservability,
  type HelperSessionCaptureMode,
} from '../../shared/helper/protocol';
import {
  type HelperReadiness,
  type HelperReadinessReason,
} from '../../shared/schemas/helper-readiness';
import {
  REQUEST_TIMEOUT_MS,
  SHUTDOWN_EXIT_MARGIN_MS,
  type HelperClientOptions,
} from './helper-client-options';
import { saturatingSafeIncrement } from './helper-client-diagnostics';
import { HelperClientRuntime } from './helper-client-runtime';

export { HelperClientError } from './helper-client-error';
export {
  ACTIVATION_CAPTURE_ROLLBACK_ENV,
  activationCaptureRollbackEnabled,
  type HelperClientOptions,
  type HelperRuntimeObservabilitySource,
} from './helper-client-options';
export type { HelperOwnerConnectionDiagnostic } from './helper-client-diagnostics';

export class HelperClient {
  readonly #runtime: HelperClientRuntime;

  constructor(options: HelperClientOptions) {
    this.#runtime = new HelperClientRuntime(options);
  }

  get readiness(): HelperReadiness {
    return this.#runtime.readiness;
  }

  /** Privacy-safe native launcher failure classification for installed diagnostics. */
  get nativeLaunchFailure(): string | null {
    return this.#runtime.nativeLaunchFailure;
  }

  /** Effective helper acknowledgement; null while no helper session is authoritative. */
  get activationCaptureEnabled(): boolean | null {
    return this.#runtime.activation.effectiveEnabled;
  }

  /** Owner-lease capability from the protocol-v10 keyboardCapture handshake. */
  get sessionKeyCaptureAvailable(): boolean | null {
    return this.#runtime.sessionKeyCaptureAvailable;
  }

  subscribeReadiness(listener: (readiness: HelperReadiness) => void): () => void {
    this.#runtime.readinessListeners.add(listener);
    return () => this.#runtime.readinessListeners.delete(listener);
  }

  subscribeNotifications(listener: (notification: HelperNotification) => void): () => void {
    this.#runtime.notificationListeners.add(listener);
    return () => this.#runtime.notificationListeners.delete(listener);
  }

  subscribeInputDeviceInvalidations(listener: () => void): () => void {
    return this.subscribeNotifications((notification) => {
      if (notification.method === 'audio.input_devices_changed') listener();
    });
  }

  async start(): Promise<void> {
    const revision = ++this.#runtime.runIntentRevision;
    this.#runtime.desiredRunning = true;
    this.#runtime.clearRestart();
    await this.#runtime.startForIntent(revision);
  }

  async restart(): Promise<void> {
    const revision = ++this.#runtime.runIntentRevision;
    this.#runtime.desiredRunning = true;
    const stopping = this.#runtime.stopOperation;
    if (stopping !== null) await stopping;
    if (!this.#runtime.intentIsCurrent(revision)) return;
    this.#runtime.clearRestart();
    const child = this.#runtime.child;
    if (child !== null) {
      const close = waitForClose(child);
      try {
        this.#runtime.terminateCurrent('unexpected-exit', true);
        const closed = await waitForCloseWithin(
          close.promise,
          this.#runtime.shutdownWaitMs + SHUTDOWN_EXIT_MARGIN_MS,
        );
        if (!closed) {
          throw new HelperClientError(
            'transport-error',
            'Native helper restart could not confirm process exit',
          );
        }
        if (!this.#runtime.intentIsCurrent(revision)) return;
        this.#runtime.clearRestart();
      } finally {
        close.cancel();
      }
    }
    await this.#runtime.startForIntent(revision);
  }

  async stop(options: { readonly requireNeutral?: boolean } = {}): Promise<void> {
    this.#runtime.runIntentRevision += 1;
    this.#runtime.desiredRunning = false;
    this.#runtime.failureTimes = [];
    this.#runtime.crashLoopOpen = false;
    this.#runtime.halfOpenProbe = false;
    await this.#runtime.stopCurrentProcess(options.requireNeutral === true);
  }

  configureActivation(enabled: boolean, bindings: readonly ActivationBinding[]) {
    if (this.#runtime.maintenancePreparing || this.#runtime.maintenancePrepared) {
      return Promise.reject(
        new HelperClientError('not-running', 'Native helper is in maintenance'),
      );
    }
    return this.#runtime.activation.configure(enabled, bindings);
  }

  async beginPhysicalObservation(): Promise<HelperRuntimeObservability> {
    if (this.#runtime.options.platform !== 'win32') {
      throw new HelperClientError(
        'not-running',
        'Passive registered-input observation is unavailable on this platform',
      );
    }
    // First confirm the owner has applied disabled activation with retained
    // bindings. Only then take the baseline, so a chord pressed during the
    // transition is historical and cannot satisfy the armed observation.
    await this.#runtime.activation.beginPhysicalObservation();
    return this.getRuntimeObservability();
  }

  samplePhysicalObservation(): Promise<HelperRuntimeObservability> {
    return this.getRuntimeObservability();
  }

  async endPhysicalObservation(): Promise<void> {
    await this.#runtime.activation.endPhysicalObservation();
  }

  setSessionCapture(mode: HelperSessionCaptureMode) {
    if (!this.#runtime.ordinaryRequestsAvailable()) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is initializing'));
    }
    if (this.#runtime.sessionKeyCaptureAvailable === false) {
      return Promise.resolve({ mode: 'off' as const });
    }
    return this.request('session.set_capture', { mode });
  }

  async resetSessionCapture(signal?: AbortSignal): Promise<void> {
    if (this.#runtime.sessionKeyCaptureAvailable === false) return;
    const revision = this.#runtime.runIntentRevision;
    this.#runtime.assertResetAllowed(revision, signal);
    const stopOnAbort = (): void => {
      void this.stop().catch(() => undefined);
    };
    signal?.addEventListener('abort', stopOnAbort, { once: true });
    try {
      await this.#runtime.stopCurrentProcess();
      this.#runtime.assertResetAllowed(revision, signal);
      if (this.#runtime.child !== null) {
        throw new HelperClientError(
          'transport-error',
          'Native helper capture reset could not confirm process exit',
        );
      }
      await this.#runtime.startForIntent(revision);
      this.#runtime.assertResetAllowed(revision, signal);
      if (this.#runtime.readiness.status !== 'ready') {
        throw new HelperClientError('not-running', 'Native helper capture reset is unavailable');
      }
      await this.setSessionCapture('off');
      this.#runtime.assertResetAllowed(revision, signal);
    } catch (error: unknown) {
      if (signal?.aborted === true || !this.#runtime.desiredRunning) {
        await this.#runtime.stopCurrentProcess().catch(() => undefined);
      }
      throw error;
    } finally {
      signal?.removeEventListener('abort', stopOnAbort);
    }
  }

  injectPaste(
    activationContext: Readonly<HelperActivationContext>,
    expectedClipboardSha256: string,
    signal?: AbortSignal,
    onCommitted?: () => void,
  ): Promise<HelperPasteResult> {
    return this.request(
      'paste.inject',
      { ...activationContext, expectedClipboardSha256 },
      REQUEST_TIMEOUT_MS,
      signal,
      onCommitted,
    );
  }

  getFrontApp(): Promise<HelperFrontApp> {
    return this.request('front_app.get', {});
  }

  getPermissions(): Promise<HelperPermissions> {
    if (!this.#runtime.ordinaryRequestsAvailable()) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is initializing'));
    }
    return this.#runtime.refreshHealthCoalesced(this.#runtime.rpcSession);
  }

  async getRuntimeObservability(): Promise<HelperRuntimeObservability> {
    const observability = await this.request('runtime.observability', {});
    const enriched = HelperRuntimeObservabilitySchema.parse({
      ...observability,
      registeredInput: {
        ...observability.registeredInput,
        electronReceived: this.#runtime.electronRegisteredObservations,
        observationAccepted: this.#runtime.physicalObservationsAccepted,
      },
    });
    this.#runtime.publishRuntimeObservability(enriched, 'runtime');
    return enriched;
  }

  recordObservationAccepted(): void {
    this.#runtime.physicalObservationsAccepted = saturatingSafeIncrement(
      this.#runtime.physicalObservationsAccepted,
    );
  }

  prepareOwnerMaintenance(
    params: HelperPrepareMaintenanceParams,
    timeoutMs: number,
    signal?: AbortSignal,
  ) {
    return this.#runtime.prepareOwnerMaintenance(params, timeoutMs, signal);
  }

  ping() {
    return this.request('ping', {});
  }

  request<Method extends HelperMethod>(
    method: Method,
    params: HelperParams<Method>,
    timeoutMs = REQUEST_TIMEOUT_MS,
    signal?: AbortSignal,
    onPasteCommitted?: () => void,
    timeoutReason: HelperReadinessReason = 'request-timeout',
    allowStopping = false,
    supervision = false,
  ): Promise<HelperResult<Method>> {
    if (signal?.aborted === true) {
      return Promise.reject(new DOMException('Native helper request cancelled', 'AbortError'));
    }
    const session = this.#runtime.rpcSession;
    if (
      session === null ||
      (!allowStopping && !this.#runtime.ordinaryRequestsAvailable()) ||
      (!this.#runtime.desiredRunning && !allowStopping)
    ) {
      return Promise.reject(new HelperClientError('not-running', 'Native helper is terminating'));
    }
    return this.#runtime.rpcChannel.request(session, method, params, {
      timeoutMs: Math.min(timeoutMs, REQUEST_TIMEOUT_MS),
      timeoutReason,
      signal,
      onPasteCommitted,
      allowDraining: allowStopping,
      supervision,
    });
  }
}
