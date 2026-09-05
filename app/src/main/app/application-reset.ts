import { app } from 'electron';
import { randomUUID } from 'node:crypto';
import { prepareResetSafely } from '../data/reset-preparation';
import type { ApplicationRuntime } from './application-runtime';
import { LIFECYCLE_TIMEOUT_MS } from './application-runtime';
import type { ApplicationShutdown } from './application-shutdown';

const RESET_ACKNOWLEDGEMENT_TIMEOUT_MS = 1_000;

export class ApplicationReset {
  readonly #runtime: ApplicationRuntime;
  readonly #shutdown: ApplicationShutdown;

  constructor(runtime: ApplicationRuntime, shutdown: ApplicationShutdown) {
    this.#runtime = runtime;
    this.#shutdown = shutdown;
  }

  async prepareDataReset(): Promise<string> {
    if (
      this.#runtime.lifecycle !== 'running' ||
      this.#runtime.dataLifecycle === null ||
      this.#runtime.resetPending
    ) {
      throw new Error('Application data reset is unavailable');
    }
    // This synchronous gate runs before the first await. It removes every mutating IPC handler and
    // aborts provider/session work; only the typed, role-authorized one-time acknowledgement stays.
    this.#runtime.resetPending = true;
    const deadline = Date.now() + LIFECYCLE_TIMEOUT_MS;
    this.#runtime.resetDeadline = deadline;
    const acknowledgementToken = randomUUID();
    this.#runtime.resetAcknowledgementToken = acknowledgementToken;
    await prepareResetSafely({
      journal: this.#runtime.dataLifecycle,
      quiesce: () => this.#shutdown.quiesce(true),
      criticalSteps: this.#shutdown.createDrainSteps(['data:reset-all']),
      deadline,
      onAbort: (restartWithoutReset, abortDeadline) =>
        this.#abortAfterFailedReset(restartWithoutReset, abortDeadline),
    });
    if (!this.#runtime.resetRestartScheduled) {
      this.#runtime.resetRestartScheduled = true;
      // Keep the renderer paint/ack window inside the reset deadline. Relaunch is forced even if
      // the renderer is hung.
      setTimeout(
        () => this.#completeResetRelaunch(),
        Math.max(0, Math.min(RESET_ACKNOWLEDGEMENT_TIMEOUT_MS, deadline - Date.now())),
      );
    }
    return acknowledgementToken;
  }

  #abortAfterFailedReset(restartWithoutReset: boolean, deadline: number): void {
    // A timed-out producer may still be executing. The reset deadline remains the final
    // cancellation edge and prevents Chromium or an audio driver from holding the process open.
    if (restartWithoutReset) app.relaunch({ args: process.argv.slice(1) });
    this.#shutdown.requestQuit({ deadline, skipDependentShutdown: true });
  }

  acknowledgeDataReset(token: string): void {
    if (
      this.#runtime.resetAcknowledgementToken === null ||
      token !== this.#runtime.resetAcknowledgementToken
    ) {
      throw new Error('Reset acknowledgement is invalid or already consumed');
    }
    this.#runtime.resetAcknowledgementToken = null;
    this.#completeResetRelaunch();
  }

  #completeResetRelaunch(): void {
    if (
      this.#runtime.dataLifecycle?.resetPrepared !== true ||
      !this.#runtime.resetRestartScheduled ||
      this.#runtime.resetDeadline === null
    ) {
      return;
    }
    this.#runtime.resetRestartScheduled = false;
    this.#runtime.resetAcknowledgementToken = null;
    app.relaunch({ args: process.argv.slice(1) });
    this.#shutdown.requestQuit({ deadline: this.#runtime.resetDeadline });
  }
}
