import { app, shell } from 'electron';
import { writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { scavengeSessionArtifacts } from '../echo/session-artifacts';
import { WhisperClientError } from '../transcription';
import type { StartupCleanupStack } from './lifecycle';
import type { ApplicationRuntime } from './application-runtime';
import type { ApplicationShutdown } from './application-shutdown';
import type { ApplicationStartupHooks } from './application-startup';
import type { StartupFoundation } from './application-startup-foundation';
import type { StartupServices } from './application-startup-services';
import type { StartupInteraction } from './application-startup-interaction';
import { localMacosUpdate } from './application-local-update';

export async function completeStartup(
  runtime: ApplicationRuntime,
  cleanup: StartupCleanupStack,
  hooks: ApplicationStartupHooks,
  shutdown: ApplicationShutdown,
  foundation: StartupFoundation,
  services: StartupServices,
  interaction: StartupInteraction,
): Promise<void> {
  const { sourceHarness, paths, diagnostics } = foundation;
  const {
    historyService,
    whisper,
    updates,
    applicationUpdates,
    helper,
    task6Composition,
    macosUpdateCoordinator,
  } = services;
  const { windows, echo, packagedMediaReady } = interaction;
  // Every consumer and the hidden non-focusable widget exist before the native gateway can
  // enable activation. A fresh helper starts disabled, then Echo applies authoritative settings.
  await windows.createAll();
  runtime.assertStartupActive();
  if (task6Composition === null) await helper.start();
  services.markHelperStartupComplete();
  runtime.assertStartupActive();
  if (app.isPackaged) {
    void updates
      .check(app.getVersion(), runtime.startupAbort.signal)
      .then(async (result) => {
        const updateState = await applicationUpdates.acceptCheckResult(result);
        if (updateState.phase === 'available' || updateState.phase === 'downloading') {
          windows.showMain();
        }
      })
      .catch(() => undefined);
  }
  // Native activation remains disabled until every eager renderer has loaded, so a startup
  // shortcut cannot begin a session whose preloaded widget or capture surface is unavailable.
  await echo.initialize();
  if (task6Composition !== null) {
    runtime.ownRuntimeDisposer(
      cleanup,
      'task6-test-driver',
      sourceHarness.bindAndExposeTask6(task6Composition, echo),
    );
    packagedMediaReady?.armAfterEchoBinding();
  }
  // Cleanup that can enumerate thousands of files starts only after the first usable renderer
  // is shown, and yields between bounded batches so it cannot monopolize the main thread.
  // Maintenance is best-effort once the usable surfaces are live. A locked stale artifact must
  // not tear down an otherwise healthy app; cancellation is still observed by the lifecycle
  // check immediately afterward.
  await Promise.allSettled([
    scavengeSessionArtifacts(paths.sessionTemporary, 64, runtime.startupAbort.signal),
    historyService.pruneAtStartupDeferred(64, runtime.startupAbort.signal),
  ]);
  runtime.assertStartupActive();
  if (process.env.TALKING_QUILL_VERIFY_WHISPER_RUNTIME === '1') {
    let code = 'ready';
    try {
      await whisper.checkWorkerModel('Xenova/whisper-small');
    } catch (error: unknown) {
      code = error instanceof WhisperClientError ? error.code : 'INTERNAL';
    }
    await writeFile(
      join(paths.temporary, 'whisper-runtime-check.json'),
      `${JSON.stringify({ code })}\n`,
      { encoding: 'utf8', mode: 0o600 },
    );
    runtime.assertStartupActive();
  }
  if (diagnostics.enabled) {
    await helper.getRuntimeObservability().catch(() => undefined);
  }
  runtime.assertStartupActive();
  runtime.lifecycle = 'running';
  hooks.acknowledgeWindowsUpdateRelaunches();
  if (process.env.NODE_ENV === 'test') {
    Reflect.set(globalThis, '__talkingQuillRequestQuit', hooks.testQuitRequest);
  }
  const localUpdate = process.argv.find((argument) => argument.startsWith('--update-local-owner='));
  const localRollback = process.argv.find((argument) =>
    argument.startsWith('--rollback-local-owner='),
  );
  const localOwnerMaintenanceRequested =
    localUpdate !== undefined ||
    localRollback !== undefined ||
    process.argv.includes('--uninstall-local-owner');
  if (app.isPackaged && process.platform === 'darwin' && localOwnerMaintenanceRequested) {
    if (macosUpdateCoordinator === null) {
      throw new Error('The installed macOS owner maintenance coordinator is unavailable');
    }
    if (localUpdate !== undefined) {
      const archive = localUpdate.slice('--update-local-owner='.length);
      await macosUpdateCoordinator.prepareUpdate(localMacosUpdate(archive));
      shutdown.quit();
      return;
    }
    if (localRollback !== undefined) {
      const archive = localRollback.slice('--rollback-local-owner='.length);
      await macosUpdateCoordinator.prepareRollback(localMacosUpdate(archive));
      shutdown.quit();
      return;
    }
    if (process.argv.includes('--uninstall-local-owner')) {
      await macosUpdateCoordinator.prepareUninstall(() =>
        shell.trashItem(dirname(dirname(process.resourcesPath))),
      );
      shutdown.quit();
      return;
    }
  }
  await diagnostics
    .record('application.started', {
      component: 'application',
      outcome: 'ready',
      appVersion: app.getVersion(),
      runtimeVersion: app.getVersion(),
    })
    .catch(() => undefined);
}
