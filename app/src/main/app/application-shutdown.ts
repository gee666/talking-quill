import { app } from 'electron';
import type { InvokeChannel } from '../../shared/ipc/registry';
import {
  type LifecycleProgress,
  type LifecycleStep,
  reportLifecycleDiagnostics,
  runBoundedLifecycle,
  runSynchronousLifecycle,
} from './lifecycle';
import { createBoundedElectronQuit } from './electron-quit';
import { createApplicationDrainSteps } from './shutdown-steps';
import type { ApplicationRuntime } from './application-runtime';
import { LIFECYCLE_TIMEOUT_MS } from './application-runtime';

export class ApplicationShutdown {
  readonly #testQuitRequest: () => void;
  readonly #runtime: ApplicationRuntime;

  constructor(runtime: ApplicationRuntime, testQuitRequest: () => void) {
    this.#runtime = runtime;
    this.#testQuitRequest = testQuitRequest;
  }

  requestUpdateInstall(): void {
    if (this.#runtime.lifecycle !== 'running' || this.#runtime.applicationUpdates === null) return;
    this.#runtime.updateInstallRequested = true;
    this.quit();
  }

  quit(deadline?: number): void {
    const effectiveDeadline = deadline ?? this.#runtime.resetDeadline;
    if (effectiveDeadline === null) this.requestQuit();
    else this.requestQuit({ deadline: effectiveDeadline });
  }

  requestQuit(
    options: { readonly deadline?: number; readonly skipDependentShutdown?: boolean } = {},
  ): void {
    if (options.skipDependentShutdown === true) this.#runtime.skipDependentShutdown = true;
    if (this.#runtime.quitPromise !== null) return;
    this.#runtime.lifecycle = 'stopping';
    this.#runtime.quitDeadline = options.deadline ?? Date.now() + LIFECYCLE_TIMEOUT_MS;
    void this.#runtime.diagnostics
      ?.record('application.stopping', { component: 'application', outcome: 'requested' })
      .catch(() => undefined);
    this.#runtime.startupAbort.abort();
    this.quiesce();
    // Start durable barriers before a broken renderer IPC, audio driver, or helper can consume the
    // remaining process deadline. The ordered drain below still awaits these same promises at the
    // normal settings and vault steps.
    this.#runtime.settingsFlush = this.#runtime.settings?.flush() ?? Promise.resolve();
    this.#runtime.vaultFlush = this.#runtime.vault?.flush() ?? Promise.resolve();
    void this.#runtime.settingsFlush.catch(() => undefined);
    void this.#runtime.vaultFlush.catch(() => undefined);
    this.#runtime.boundedQuit = createBoundedElectronQuit(app, this.#runtime.quitDeadline, {
      fallbackExitCode: 1,
      onDeadline: () => this.#forceQuitAtDeadline(),
    });
    this.#runtime.quitPromise = this.#drainBeforeQuit();
  }

  handleBeforeQuit(event: Electron.Event): void {
    if (this.#runtime.quitAllowed) {
      this.shutdown();
      return;
    }
    event.preventDefault();
    this.quit();
  }

  shutdown(): void {
    if (this.#runtime.shutdownComplete) return;
    this.#runtime.shutdownComplete = true;
    const runtimeDisposers = this.#runtime.runtimeDisposers.splice(0).reverse();
    const diagnostics = runSynchronousLifecycle('shutdown', [
      { name: 'quiesce', run: () => this.quiesce() },
      { name: 'model-events', run: () => this.#runtime.removeModelEvents?.() },
      { name: 'helper-readiness', run: () => this.#runtime.removeHelperReadiness?.() },
      ...runtimeDisposers.map((dispose, index) => ({
        name: `runtime-disposer-${String(index + 1)}`,
        run: dispose,
      })),
      { name: 'ipc-dispose', run: () => this.#runtime.ipc?.dispose() },
      { name: 'tray', run: () => this.#runtime.tray?.destroy() },
      { name: 'windows', run: () => this.#runtime.windows?.destroyAll() },
      ...(this.#runtime.skipDependentShutdown
        ? []
        : [{ name: 'history', run: () => this.#runtime.history?.close() }]),
    ]);
    reportLifecycleDiagnostics(diagnostics);
    this.#runtime.clearOwnedReferences();
    if (Reflect.get(globalThis, '__talkingQuillRequestQuit') === this.#testQuitRequest) {
      Reflect.deleteProperty(globalThis, '__talkingQuillRequestQuit');
    }
    this.#runtime.lifecycle = 'stopped';
  }

  async #drainBeforeQuit(): Promise<void> {
    if (!(await this.#waitForStartupSettlement())) {
      // Startup cleanup is cancellation-aware, but an OS filesystem call can still stall. Never
      // let that hold the process open indefinitely or race asynchronous startup against teardown.
      this.#runtime.skipDependentShutdown = true;
      this.#finishQuit(1);
      return;
    }
    const diagnostics = await runBoundedLifecycle(
      'shutdown',
      this.createDrainSteps(),
      LIFECYCLE_TIMEOUT_MS,
      {
        deadline: this.#runtime.quitDeadline,
        onProgress: (progress) => this.#observeShutdownProgress(progress),
      },
    );
    if (diagnostics.some(({ outcome }) => outcome === 'timed-out')) {
      this.#runtime.skipDependentShutdown = true;
    }
    reportLifecycleDiagnostics(diagnostics);
    this.#runtime.quitAllowed = true;
    if (this.#runtime.updateInstallRequested) {
      try {
        if (this.#runtime.applicationUpdates === null)
          throw new Error('Update installer is unavailable');
        this.#runtime.applicationUpdates.quitAndInstall();
        return;
      } catch {
        this.#runtime.updateInstallRequested = false;
      }
    }
    // All application-owned producers and durable stores have settled. Do not hand control back
    // to Chromium's graceful audio teardown, which can wait forever in a native driver.
    this.#finishQuit(diagnostics.some(({ outcome }) => outcome === 'timed-out') ? 1 : 0);
  }

  #observeShutdownProgress(progress: LifecycleProgress): void {
    this.#runtime.shutdownProgress = progress;
    if (process.env.NODE_ENV === 'test') {
      const snapshot = Object.freeze({ ...progress });
      Reflect.set(globalThis, '__talkingQuillShutdownProgress', snapshot);
      console.error('Talking Quill shutdown progress', snapshot);
    }
  }

  #forceQuitAtDeadline(): void {
    this.#runtime.skipDependentShutdown = true;
    console.error('Talking Quill shutdown deadline expired', {
      step: this.#runtime.shutdownProgress?.step ?? 'startup-settlement',
      pendingIpc: this.#runtime.ipc?.pendingChannels().slice(0, 16) ?? [],
    });
    this.shutdown();
  }

  #finishQuit(exitCode: number): void {
    if (this.#runtime.processExitRequested) return;
    this.#runtime.processExitRequested = true;
    this.#runtime.quitAllowed = true;
    this.#runtime.boundedQuit?.request(exitCode);
  }

  async #waitForStartupSettlement(): Promise<boolean> {
    const startup = this.#runtime.startPromise;
    if (startup === null) return true;
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      return await Promise.race([
        startup.then(
          () => true,
          () => true,
        ),
        new Promise<boolean>((resolveWait) => {
          timer = setTimeout(
            () => resolveWait(false),
            Math.max(1, this.#runtime.quitDeadline - Date.now()),
          );
          timer.unref();
        }),
      ]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  quiesce(preserveResetAcknowledgement = false): void {
    this.#runtime.windows?.beginQuit();
    this.#runtime.tray?.stopAccepting();
    this.#runtime.ipc?.stopAccepting(
      preserveResetAcknowledgement ? ['data:reset-renderer-ack'] : [],
    );
    this.#runtime.providerMutations?.stopAccepting();
    this.#runtime.providerOperations?.dispose();
    this.#runtime.updateOperations?.dispose();
    this.#runtime.echo?.abort('shutdown');
    this.#runtime.providers?.dispose();
  }

  createDrainSteps(excludedIpcChannels: readonly InvokeChannel[] = []): readonly LifecycleStep[] {
    return createApplicationDrainSteps(
      {
        ipc: this.#runtime.ipc,
        tray: this.#runtime.tray,
        providerMutations: this.#runtime.providerMutations,
        echo: this.#runtime.echo,
        providers: this.#runtime.providers,
        recording: this.#runtime.recording,
        models: this.#runtime.models,
        whisper: this.#runtime.whisper,
        helper:
          this.#runtime.helper === null
            ? null
            : { stop: () => this.stopHelperAndDrainDiagnostics() },
        history: this.#runtime.history,
        settings:
          this.#runtime.settings === null
            ? null
            : {
                flush: () =>
                  settlePersistenceFlush(
                    this.#runtime.settingsFlush,
                    this.#runtime.settings?.flush(),
                  ),
              },
        vault:
          this.#runtime.vault === null
            ? null
            : {
                flush: () =>
                  settlePersistenceFlush(this.#runtime.vaultFlush, this.#runtime.vault?.flush()),
              },
        diagnostics: this.#runtime.diagnostics,
      },
      excludedIpcChannels,
    );
  }

  async stopHelperAndDrainDiagnostics(): Promise<void> {
    const failures: unknown[] = [];
    try {
      await this.#runtime.helper?.stop({ requireNeutral: true });
    } catch (error: unknown) {
      failures.push(error);
    }
    try {
      this.#runtime.removeHelperReadiness?.();
    } catch (error: unknown) {
      failures.push(error);
    } finally {
      this.#runtime.removeHelperReadiness = null;
    }
    // Diagnostic writes are best effort and may never settle. Persistence runs
    // before the diagnostic logger's independently bounded final disposal.
    if (failures.length > 0) throw failures[0];
  }
}

async function settlePersistenceFlush(
  early: Promise<void> | null,
  final: Promise<void> | undefined,
): Promise<void> {
  const outcomes = await Promise.allSettled([
    early ?? Promise.resolve(),
    final ?? Promise.resolve(),
  ]);
  const failure = outcomes.find(
    (outcome): outcome is PromiseRejectedResult => outcome.status === 'rejected',
  );
  if (failure !== undefined) throw failure.reason;
}
