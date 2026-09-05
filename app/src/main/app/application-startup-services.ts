import { app, clipboard } from 'electron';
import { existsSync } from 'node:fs';
import { dirname, join } from 'node:path';
import type { HelperReadiness } from '../../shared/schemas/helper-readiness';
import { CaptureWindowClient } from '../audio/capture-window-client';
import { RecordingService } from '../audio/recording-service';
import { HistoryService } from '../history/history-service';
import { installHelperInputDeviceRouter } from '../helper/helper-input-device-router';
import { IpcEventEmitter } from '../ipc/event-emitter';
import { ModelAccessCoordinator, ModelManager, WhisperWorkerClient } from '../transcription';
import { PinnedJsonTransport } from '../providers';
import { UpdateService } from '../info/update-service';
import { ApplicationUpdateController } from '../info/application-update-controller';
import { createElectronUpdateBackend } from '../info/electron-update-backend';
import { MacosOwnerUpdateCoordinator } from '../info/macos-owner-update-coordinator';
import { SystemInfoService } from '../info/system-info-service';
import { NoticesService } from '../info/notices-service';
import { type DiagnosticFailureCode, type DiagnosticMetadata } from '../security/diagnostic-logger';
import type { StartupCleanupStack } from './lifecycle';
import { AppStateService } from './app-state-service';
import { ModelRuntimeCoordinator } from './model-runtime-coordinator';
import type { WindowManager } from './window-manager';
import type { ApplicationRuntime } from './application-runtime';
import type { ApplicationShutdown } from './application-shutdown';
import type { ApplicationStartupHooks } from './application-startup';
import type { StartupFoundation } from './application-startup-foundation';
import { createHelper } from './application-helper';

export async function prepareServices(
  runtime: ApplicationRuntime,
  cleanup: StartupCleanupStack,
  hooks: ApplicationStartupHooks,
  shutdown: ApplicationShutdown,
  foundation: StartupFoundation,
) {
  const {
    sourceHarness,
    paths,
    helperExecutablePath,
    settings,
    diagnostics,
    history,
    microphonePermission,
    systemAudioCapture,
    observeEgress,
  } = foundation;
  let windowTarget: WindowManager | null = null;
  const events = new IpcEventEmitter(runtime.roles, () => windowTarget?.getWebContents() ?? []);
  const testNow = sourceHarness.testNow();
  const historyService = new HistoryService({
    store: history,
    settings,
    events,
    clipboard: { writeText: (text) => clipboard.writeText(text) },
    screenshotsDirectory: paths.screenshots,
    ...(testNow === null ? {} : { now: testNow }),
  });
  const modelAccess = new ModelAccessCoordinator();
  const models = new ModelManager({
    modelsDirectory: paths.models,
    temporaryDirectory: paths.modelTemporary,
    accessCoordinator: modelAccess,
    observeEgress,
  });
  runtime.models = models;
  cleanup.add('models', () => models.shutdown());
  await models.initialize();
  runtime.assertStartupActive();

  const whisper = new WhisperWorkerClient({
    cacheDirectory: paths.models,
    acquireModelUse: (modelId, signal) => models.acquireUse(modelId, signal),
  });
  runtime.whisper = whisper;
  cleanup.add('whisper-worker', () => whisper.close());
  const modelRuntime = new ModelRuntimeCoordinator({ settings, events, models, whisper });
  const removeModelEvents = modelRuntime.subscribeProgress();
  runtime.removeModelEvents = removeModelEvents;
  cleanup.add('model-events', removeModelEvents);

  const state = new AppStateService(settings, events);
  state.setModelReady((await modelRuntime.bindState(state)).state === 'ready');
  const recording = new RecordingService(
    new CaptureWindowClient(undefined, (id) => runtime.roles.get(id)?.role ?? null),
    settings,
    events,
    microphonePermission,
    systemAudioCapture,
  );
  const updates = new UpdateService(
    new PinnedJsonTransport(undefined, { category: 'update', observeEgress }),
  );
  const automaticUpdateAvailable =
    app.isPackaged &&
    ((process.platform === 'win32' && (process.arch === 'x64' || process.arch === 'arm64')) ||
      (process.platform === 'darwin' && (process.arch === 'x64' || process.arch === 'arm64'))) &&
    existsSync(join(process.resourcesPath, 'app-update.yml'));
  const installedMacosOwnerAvailable =
    app.isPackaged &&
    process.platform === 'darwin' &&
    helperExecutablePath !== null &&
    hooks.validInstalledMacosOwner(process.resourcesPath, helperExecutablePath) &&
    existsSync(join(dirname(process.resourcesPath), 'MacOS', 'talking-quill-macos-service-bridge'));
  const macosUpdateCoordinator = installedMacosOwnerAvailable
    ? new MacosOwnerUpdateCoordinator({
        helper: () => runtime.helper,
        helperExecutable: helperExecutablePath,
        installedApp: dirname(dirname(process.resourcesPath)),
        temporaryRoot: app.getPath('temp'),
      })
    : null;
  const automaticInstallAvailable =
    automaticUpdateAvailable && (process.platform !== 'darwin' || macosUpdateCoordinator !== null);
  const applicationUpdates = new ApplicationUpdateController({
    currentVersion: app.getVersion(),
    backend: automaticInstallAvailable ? createElectronUpdateBackend(process.arch) : null,
    ...(process.platform !== 'darwin' || macosUpdateCoordinator === null
      ? {}
      : { prepareInstall: (download) => macosUpdateCoordinator.prepareUpdate(download) }),
    publish: (update) => events.send('info:update-changed', update),
    requestInstall: () => shutdown.requestUpdateInstall(),
  });
  runtime.applicationUpdates = applicationUpdates;
  runtime.ownRuntimeDisposer(cleanup, 'application-updates', () => applicationUpdates.dispose());
  const systemInfo = new SystemInfoService(paths, () => microphonePermission.openSettings());
  const notices = new NoticesService(
    app.isPackaged
      ? join(process.resourcesPath, 'THIRD_PARTY_NOTICES.txt')
      : join(app.getAppPath(), 'assets', 'THIRD_PARTY_NOTICES.txt'),
  );
  runtime.recording = recording;
  cleanup.add('recording', () => recording.shutdown());

  const helper = createHelper(
    runtime,
    diagnostics.enabled ? join(paths.logs, 'owner-connection-helper-journal.json') : undefined,
  );
  if (helper === null) throw new Error('The native helper is unavailable on this platform');
  runtime.helper = helper;
  cleanup.add('helper', () => shutdown.stopHelperAndDrainDiagnostics());
  runtime.ownRuntimeDisposer(
    cleanup,
    'helper-input-device-routing',
    installHelperInputDeviceRouter({ source: helper, target: recording }),
  );
  const task6Loader = sourceHarness.loadTask6({ history, settings, recording });
  const task6Composition = task6Loader === null ? null : await task6Loader;
  const echoHelper = task6Composition?.helper ?? helper;
  if (task6Composition !== null) state.setModelReady(true);
  let lastHelperDiagnostic = '';
  let helperStartupComplete = false;
  let helperDiagnosticTail = Promise.resolve();
  const observeHelperReadiness = (readiness: HelperReadiness): void => {
    state.setHelperReadiness(readiness);
    const diagnosticKey = JSON.stringify({
      status: readiness.status,
      reason: readiness.reason,
      helperVersion: readiness.helperVersion,
      permissions: readiness.permissions,
      nativeLaunchFailure: helper.nativeLaunchFailure,
    });
    const failed = readiness.status === 'unavailable' || readiness.status === 'incompatible';
    const startupFailure = !helperStartupComplete && failed;
    helperDiagnosticTail = helperDiagnosticTail
      .then(async () => {
        if (diagnosticKey === lastHelperDiagnostic) return;
        const metadata: DiagnosticMetadata = {
          component: 'helper',
          outcome: readiness.status,
          reason: readiness.reason ?? 'none',
          ...(failed && helper.nativeLaunchFailure !== null
            ? { nativeFailure: helper.nativeLaunchFailure }
            : {}),
        };
        const failureCode: DiagnosticFailureCode = `${
          startupFailure ? 'HELPER_STARTUP' : 'HELPER_RUNTIME'
        }_${readiness.status.toUpperCase()}` as DiagnosticFailureCode;
        const written = startupFailure
          ? await diagnostics.recordStartupFailure(failureCode)
          : failed
            ? await diagnostics.recordOperationalFailure(failureCode)
            : await diagnostics.record('helper.readiness.changed', metadata);
        if (written) lastHelperDiagnostic = diagnosticKey;
      })
      .catch(() => undefined);
  };
  const removeHelperReadiness = echoHelper.subscribeReadiness(observeHelperReadiness);
  // start() begins before window construction. Record the current snapshot
  // too, otherwise an immediate spawn/owner failure can precede subscription.
  observeHelperReadiness(echoHelper.readiness);
  runtime.removeHelperReadiness = removeHelperReadiness;

  return {
    events,
    historyService,
    models,
    whisper,
    modelRuntime,
    state,
    recording,
    updates,
    applicationUpdates,
    systemInfo,
    notices,
    helper,
    task6Composition,
    echoHelper,
    macosUpdateCoordinator,
    setWindowTarget: (windows: WindowManager) => {
      windowTarget = windows;
    },
    markHelperStartupComplete: () => {
      helperStartupComplete = true;
    },
  };
}

export type StartupServices = Awaited<ReturnType<typeof prepareServices>>;
