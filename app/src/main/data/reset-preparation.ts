import {
  runBoundedLifecycle,
  type LifecycleDiagnostic,
  type LifecycleStep,
} from '../app/lifecycle';

export interface ResetJournalOwner {
  prepareReset(): Promise<void>;
  cancelPreparedReset(): Promise<void>;
}

export interface ResetPreparationOptions {
  readonly journal: ResetJournalOwner;
  readonly quiesce: () => void;
  readonly criticalSteps: readonly LifecycleStep[];
  readonly timeoutMs: number;
  readonly onAbort: (restartWithoutReset: boolean) => void;
}

export class ResetPreparationError extends Error {
  constructor(
    message: string,
    readonly diagnostics: readonly LifecycleDiagnostic[] = [],
    options?: ErrorOptions,
  ) {
    super(message, options);
    this.name = 'ResetPreparationError';
  }
}

/** Quiesces synchronously, journals deletion, and requires every ordered critical drain before ack. */
export async function prepareResetSafely(options: ResetPreparationOptions): Promise<void> {
  try {
    options.quiesce();
    await options.journal.prepareReset();
    // Ordering is a security boundary: producers stop before backing stores flush/close. The
    // lifecycle runner preserves caller order while sharing one deadline across every transition.
    const diagnostics = await runBoundedLifecycle(
      'shutdown',
      options.criticalSteps,
      options.timeoutMs,
      { stopOnFailure: true },
    );
    if (diagnostics.length > 0) {
      throw new ResetPreparationError('Critical reset drain did not complete.', diagnostics);
    }
  } catch (error: unknown) {
    let restartWithoutReset = false;
    const cleanupErrors: unknown[] = [];
    try {
      await options.journal.cancelPreparedReset();
      restartWithoutReset = true;
    } catch (cleanupError: unknown) {
      cleanupErrors.push(cleanupError);
    }
    try {
      // The process is already atomically quiesced. It must terminate even if journal cleanup
      // failed; relaunch is safe only after the journal is definitely absent.
      options.onAbort(restartWithoutReset);
    } catch (abortError: unknown) {
      cleanupErrors.push(abortError);
    }
    if (cleanupErrors.length === 0) {
      if (error instanceof ResetPreparationError) throw error;
      throw new ResetPreparationError('Application data reset preparation failed.', [], {
        cause: error,
      });
    }
    const message =
      error instanceof ResetPreparationError
        ? error.message
        : 'Application data reset preparation failed.';
    const diagnostics = error instanceof ResetPreparationError ? error.diagnostics : [];
    throw new ResetPreparationError(message, diagnostics, {
      cause: new AggregateError(
        [error, ...cleanupErrors],
        'Reset preparation and abort cleanup both failed.',
      ),
    });
  }
}
