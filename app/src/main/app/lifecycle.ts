import { randomUUID } from 'node:crypto';

export type LifecyclePhase = 'startup-rollback' | 'shutdown';
export type LifecycleOutcome = 'rejected' | 'timed-out';

export interface LifecycleDiagnostic {
  readonly phase: LifecyclePhase;
  readonly step: string;
  readonly outcome: LifecycleOutcome;
}

export interface LifecycleStep {
  readonly name: string;
  readonly run: () => void | Promise<void>;
}

const DEFAULT_CLEANUP_TIMEOUT_MS = 5_000;

export class StartupCancelledError extends Error {
  constructor() {
    super('Application startup was cancelled');
    this.name = 'StartupCancelledError';
  }
}

export class StartupCleanupStack {
  readonly #steps: LifecycleStep[] = [];
  #settled = false;

  add(name: string, run: LifecycleStep['run']): void {
    if (this.#settled) throw new Error('Startup cleanup ownership is already settled');
    this.#steps.push({ name, run });
  }

  disarm(): void {
    this.#settled = true;
    this.#steps.length = 0;
  }

  async rollback(timeoutMs = DEFAULT_CLEANUP_TIMEOUT_MS): Promise<readonly LifecycleDiagnostic[]> {
    if (this.#settled) return [];
    this.#settled = true;
    return runBoundedLifecycle('startup-rollback', [...this.#steps].reverse(), timeoutMs);
  }
}

export function runSynchronousLifecycle(
  phase: LifecyclePhase,
  steps: readonly LifecycleStep[],
): readonly LifecycleDiagnostic[] {
  const diagnostics: LifecycleDiagnostic[] = [];
  for (const step of steps) {
    try {
      const result = step.run();
      if (result instanceof Promise) {
        void result.catch(() => undefined);
        diagnostics.push({ phase, step: step.name, outcome: 'rejected' });
      }
    } catch {
      diagnostics.push({ phase, step: step.name, outcome: 'rejected' });
    }
  }
  return Object.freeze(diagnostics);
}

export async function runBoundedLifecycle(
  phase: LifecyclePhase,
  steps: readonly LifecycleStep[],
  timeoutMs = DEFAULT_CLEANUP_TIMEOUT_MS,
  options: { readonly stopOnFailure?: boolean } = {},
): Promise<readonly LifecycleDiagnostic[]> {
  const diagnostics: LifecycleDiagnostic[] = [];
  const deadline = Date.now() + Math.max(1, timeoutMs);
  for (let index = 0; index < steps.length; index += 1) {
    const step = steps[index];
    if (step === undefined) continue;
    const remainingSteps = steps.length - index;
    const remainingMs = Math.max(1, deadline - Date.now());
    const stepBudgetMs = Math.max(1, Math.floor(remainingMs / remainingSteps));
    const outcome = await settleBounded(step.run, stepBudgetMs);
    if (outcome !== null) {
      diagnostics.push({ phase, step: step.name, outcome });
      if (options.stopOnFailure === true) break;
    }
  }
  return Object.freeze(diagnostics);
}

export interface FatalStartupReport {
  readonly code: 'STARTUP_FAILED';
  readonly diagnosticId: string;
  readonly message: string;
}

export function createFatalStartupReport(): FatalStartupReport {
  return Object.freeze({
    code: 'STARTUP_FAILED',
    diagnosticId: randomUUID(),
    message: 'Talking Quill could not start. No diagnostic report was saved.',
  });
}

export function reportLifecycleDiagnostics(
  diagnostics: readonly LifecycleDiagnostic[],
  diagnosticId = randomUUID(),
): void {
  if (diagnostics.length === 0) return;
  console.error('Talking Quill lifecycle cleanup incomplete', {
    diagnosticId,
    diagnostics: diagnostics.slice(0, 16),
  });
}

async function settleBounded(
  operation: LifecycleStep['run'],
  timeoutMs: number,
): Promise<LifecycleOutcome | null> {
  let resolveTimeout!: (outcome: 'timed-out') => void;
  const timeout = new Promise<'timed-out'>((resolve) => {
    resolveTimeout = resolve;
  });
  const timer = setTimeout(() => resolveTimeout('timed-out'), timeoutMs);
  const task = Promise.resolve()
    .then(operation)
    .then(
      () => 'fulfilled' as const,
      () => 'rejected' as const,
    );
  const outcome = await Promise.race([task, timeout]);
  clearTimeout(timer);
  return outcome === 'fulfilled' ? null : outcome;
}
