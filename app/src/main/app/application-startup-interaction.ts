import { app, powerMonitor, shell } from 'electron';
import { EchoSessionController } from '../echo/echo-session-controller';
import { createInsertionService } from '../insertion/insertion-service';
import { ScreenshotService } from '../screenshot/screenshot-service';
import { SmartTranscriptionService } from '../smart/smart-transcription-service';
import { installHelperWakeRevalidator } from '../helper/helper-wake-revalidator';
import { createHandlers } from '../ipc/handlers';
import { registerIpcTransport } from '../ipc/transport';
import { WelcomeService } from '../welcome/welcome-service';
import { SettingsTransferFileService } from '../data/settings-transfer-file-service';
import type { StartupCleanupStack } from './lifecycle';
import { TrayController } from './tray-controller';
import { WindowManager } from './window-manager';
import { WidgetCaptureExclusion } from './widget-capture-exclusion';
declare const __TALKING_QUILL_SOURCE_REVISION__: string;
import type { ApplicationRuntime } from './application-runtime';
import type { ApplicationShutdown } from './application-shutdown';
import type { ApplicationReset } from './application-reset';
import type { StartupFoundation } from './application-startup-foundation';
import type { StartupServices } from './application-startup-services';

export function prepareInteraction(
  runtime: ApplicationRuntime,
  cleanup: StartupCleanupStack,
  shutdown: ApplicationShutdown,
  reset: ApplicationReset,
  foundation: StartupFoundation,
  services: StartupServices,
) {
  const {
    sourceHarness,
    paths,
    settings,
    launchAtLogin,
    commands,
    vocabulary,
    vocabularyFiles,
    providerConfigs,
    piInstallation,
    providers,
    providerMutations,
    providerOperations,
    updateOperations,
    loader,
  } = foundation;
  const {
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
  } = services;
  const windows = new WindowManager(loader, runtime.roles, settings, {
    requestQuit: () => shutdown.quit(),
    onMaximizedChanged: (maximized) => events.send('window:maximized-changed', { maximized }),
    onMainHidden: () => {
      void recording.stopTest();
    },
    showMainOnFirstLoad: !runtime.windowsLoginStart && !app.getLoginItemSettings().wasOpenedAtLogin,
  });
  runtime.windows = windows;
  if (runtime.showMainWhenReady) {
    runtime.showMainWhenReady = false;
    windows.showMainByUser();
  }
  cleanup.add('windows', () => windows.destroyAll());
  services.setWindowTarget(windows);
  const widgetCaptureExclusion = new WidgetCaptureExclusion({
    windows,
    getWidgetSize: () => settings.get().app.widgetSize,
    getFrontApp: () => echoHelper.getFrontApp(),
  });
  const screenshots = new ScreenshotService({
    setWidgetExcluded: widgetCaptureExclusion.setExcluded,
  });
  const smart = new SmartTranscriptionService({
    settings,
    configs: providerConfigs,
    providers,
    screenshots: task6Composition?.screenshots ?? screenshots,
    helper: echoHelper,
    screenshotsDirectory: paths.screenshots,
  });
  const echo = new EchoSessionController({
    settings,
    platform: process.platform === 'darwin' ? 'darwin' : 'win32',
    recording: task6Composition?.recording ?? recording,
    whisper: task6Composition?.whisper ?? whisper,
    helper: echoHelper,
    insertion: task6Composition?.insertion ?? createInsertionService(helper),
    commands,
    history: historyService,
    smartProcessor: smart,
    windows,
    events,
    sound: () => shell.beep(),
    isModelReady: () => state.modelReady,
    acquireModelUse:
      task6Composition === null
        ? (modelId, signal) => models.acquireUse(modelId, signal)
        : () =>
            Promise.resolve({
              status: { state: 'ready' },
              release: () => undefined,
            }),
  });
  runtime.echo = echo;
  runtime.ownRuntimeDisposer(
    cleanup,
    'helper-wake-revalidation',
    installHelperWakeRevalidator({
      source: powerMonitor,
      isSafeToRevalidate: () =>
        runtime.lifecycle === 'running' &&
        helper.readiness.status === 'ready' &&
        echo.systemWakeRevalidationSafe,
      recycle: () => helper.resetSessionCapture(),
    }),
  );
  cleanup.add('echo-model-readiness-target', modelRuntime.bindEcho(echo));
  cleanup.add('echo-session', () => echo.shutdown());
  const welcome = new WelcomeService(settings, {
    microphoneReady: () =>
      task6Composition?.welcome.microphone ?? recording.microphoneReadyForWelcome(),
    microphoneObservation: () =>
      task6Composition?.welcome.microphone
        ? { boundDeviceId: 'source-e2e-microphone', observedRms: 0.2, sampleCount: 3_200 }
        : recording.microphoneTestObservation(),
    modelReady: () =>
      task6Composition === null
        ? modelRuntime.selectedModelReadyForWelcome()
        : Promise.resolve(task6Composition.welcome.model),
    modelRevision: (modelId) => modelRuntime.manifestRevision(modelId),
  });
  const removeModelWelcomeTarget = modelRuntime.bindWelcome(welcome);
  recording.setWelcomeEvidenceInvalidator(() =>
    welcome.invalidateMicrophoneBinding().catch(() => undefined),
  );
  recording.setWelcomeEvidenceValidationListener((known) => {
    if (known) welcome.confirmMicrophoneBinding();
    else welcome.beginMicrophoneBindingValidation();
  });
  cleanup.add('welcome-readiness-target', removeModelWelcomeTarget);
  const removeEchoState = echo.subscribe((snapshot) => state.setSession(snapshot));
  runtime.ownRuntimeDisposer(cleanup, 'echo-state', removeEchoState);
  const removeSelectedModel = modelRuntime.subscribeSelectedModel(task6Composition !== null);
  runtime.ownRuntimeDisposer(cleanup, 'selected-model', removeSelectedModel);

  const tray = new TrayController(state, {
    showMain: () => windows.showMainByUser(),
    quit: () => shutdown.quit(),
    setEnabled: async (enabled) => {
      await echo.updateGeneral({ app: { enabled } });
    },
  });
  runtime.tray = tray;
  cleanup.add('tray', async () => {
    tray.stopAccepting();
    await tray.drain();
    tray.destroy();
  });
  let trayEnabled = settings.get().app.enabled;
  const removeTraySettings = settings.subscribe((next) => {
    if (next.app.enabled === trayEnabled) return;
    trayEnabled = next.app.enabled;
    tray.refresh();
  });
  runtime.ownRuntimeDisposer(cleanup, 'tray-settings', removeTraySettings);

  const packagedMediaReady = sourceHarness.createPackagedMediaReady(task6Composition);
  const settingsTransferFiles = new SettingsTransferFileService(commands, echo);

  const ipc = registerIpcTransport(
    runtime.roles,
    createHandlers({
      appVersion: app.getVersion(),
      sourceRevision: __TALKING_QUILL_SOURCE_REVISION__,
      platform: process.platform,
      state,
      launchAtLogin,
      providerConfigs,
      providerMutations,
      providerOperations,
      providers,
      piInstallation,
      smart,
      windows,
      models,
      recording,
      echo,
      history: historyService,
      commands,
      vocabulary,
      vocabularyFiles,
      settingsTransferFiles,
      welcome,
      updates,
      updateOperations,
      applicationUpdates,
      systemInfo,
      notices,
      diagnosticSummary: () => ({
        appVersion: app.getVersion(),
        platform: process.platform,
        architecture: process.arch,
        helper: state.getState().helper,
        nativeLaunchFailure: helper.nativeLaunchFailure,
        settings: settings.getDiagnostic(),
      }),
      ...(packagedMediaReady === undefined
        ? {}
        : { packagedMediaReady: packagedMediaReady.rendererReady }),
      requestDataReset: () => reset.prepareDataReset(),
      acknowledgeDataReset: (token) => reset.acknowledgeDataReset(token),
    }),
  );
  runtime.ipc = ipc;
  cleanup.add('ipc', async () => {
    ipc.stopAccepting();
    await ipc.drain();
    ipc.dispose();
  });

  return { windows, echo, packagedMediaReady };
}

export type StartupInteraction = Awaited<ReturnType<typeof prepareInteraction>>;
