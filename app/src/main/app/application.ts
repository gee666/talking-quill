import { app, clipboard, powerMonitor, safeStorage, session, shell } from 'electron';

declare const __TALKING_QUILL_SOURCE_REVISION__: string;

import { execFileSync } from 'node:child_process';
import { randomUUID } from 'node:crypto';
import { closeSync, constants, existsSync, lstatSync, openSync, readFileSync } from 'node:fs';
import { writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION } from '../../shared/constants/app';
import type { InvokeChannel } from '../../shared/ipc/registry';
import type { HelperReadiness } from '../../shared/schemas/helper-readiness';
import { CaptureWindowClient } from '../audio/capture-window-client';
import { RecordingService } from '../audio/recording-service';
import { EchoSessionController } from '../echo/echo-session-controller';
import { scavengeSessionArtifacts } from '../echo/session-artifacts';
import { HistoryService } from '../history/history-service';
import { createInsertionService } from '../insertion/insertion-service';
import { VoiceCommandStore } from '../commands/voice-command-store';
import { ScreenshotService } from '../screenshot/screenshot-service';
import { SmartTranscriptionService } from '../smart/smart-transcription-service';
import { VocabularyStore } from '../vocabulary/vocabulary-store';
import { VocabularyFileService } from '../vocabulary/file-service';
import { HelperClient, activationCaptureRollbackEnabled, resolveHelperExecutable } from '../helper';
import { installHelperInputDeviceRouter } from '../helper/helper-input-device-router';
import { installHelperWakeRevalidator } from '../helper/helper-wake-revalidator';
import { createHandlers } from '../ipc/handlers';
import { IpcEventEmitter } from '../ipc/event-emitter';
import { registerIpcTransport, type IpcTransportLifecycle } from '../ipc/transport';
import {
  ModelAccessCoordinator,
  ModelManager,
  WhisperClientError,
  WhisperWorkerClient,
} from '../transcription';
import {
  CredentialVault,
  HistoryStore,
  SETTINGS_MIGRATIONS,
  SettingsStore,
  createAppPaths,
  ensureAppDirectories,
  validateAppRootBeforeUse,
} from '../persistence';
import { ProviderOperationCoordinator, PinnedJsonTransport } from '../providers';
import type { ProviderMutationService, ProviderService } from '../providers';
import { MicrophonePermissionController } from '../security/microphone-permission';
import { SystemAudioCaptureController } from '../security/system-audio-capture';
import { WelcomeService } from '../welcome/welcome-service';
import { UpdateService } from '../info/update-service';
import { UpdateOperationCoordinator } from '../info/update-operation-coordinator';
import { ApplicationUpdateController } from '../info/application-update-controller';
import {
  createElectronUpdateBackend,
  launchWindowsUpdateReadyHelper,
} from '../info/electron-update-backend';
import { acknowledgeWindowsUpdateAppReady } from '../info/windows-update-relaunch-intent';
import {
  MacosOwnerUpdateCoordinator,
  type DownloadedApplicationUpdate,
} from '../info/macos-owner-update-coordinator';
import { parseUnsignedUpdateIdentity } from '../info/unsigned-update-identity';
import { SystemInfoService } from '../info/system-info-service';
import { NoticesService } from '../info/notices-service';
import { DataLifecycleService } from '../data/data-lifecycle-service';
import { SettingsTransferFileService } from '../data/settings-transfer-file-service';
import { createNativeOwnedTreeRemoval } from '../data/native-owned-tree-removal';
import { prepareResetSafely } from '../data/reset-preparation';
import {
  DiagnosticLogger,
  type DiagnosticFailureCode,
  type DiagnosticMetadata,
} from '../security/diagnostic-logger';
import { createEgressProofObserver } from '../security/egress-audit';
import { installApplicationProtocol } from '../security/protocol';
import { getTrustedCaptureDocument, secureSession } from '../security/session-policy';
import {
  StartupCancelledError,
  StartupCleanupStack,
  type LifecycleProgress,
  type LifecycleStep,
  reportLifecycleDiagnostics,
  runBoundedLifecycle,
  runSynchronousLifecycle,
} from './lifecycle';
import { AppStateService } from './app-state-service';
import { createBoundedElectronQuit, type BoundedElectronQuit } from './electron-quit';
import { LaunchAtLoginService } from './launch-at-login-service';
import { ModelRuntimeCoordinator } from './model-runtime-coordinator';
import { createProviderRuntime } from './provider-runtime';
import { RendererLoader, selectDevelopmentRendererUrl } from './renderer-loader';
import { createApplicationDrainSteps } from './shutdown-steps';
import { SourceE2EHarness } from './source-e2e-harness';
import { TrayController } from './tray-controller';
import { WindowManager } from './window-manager';
import { WidgetCaptureExclusion } from './widget-capture-exclusion';
import { WindowRoleRegistry } from './window-role-registry';
// Leave Pi RPC's 5.75 second retirement envelope intact after earlier producer drains.
const LIFECYCLE_TIMEOUT_MS = 15_000;
const RESET_ACKNOWLEDGEMENT_TIMEOUT_MS = 1_000;
type ApplicationLifecycle = 'new' | 'starting' | 'running' | 'stopping' | 'stopped' | 'failed';

export interface TalkingQuillApplicationOptions {
  readonly windowsLoginStart?: boolean;
  readonly packagedEgressProof?: boolean;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
}

export class TalkingQuillApplication {
  readonly #roles = new WindowRoleRegistry();
  readonly #startupAbort = new AbortController();
  readonly #windowsLoginStart: boolean;
  readonly #packagedEgressProof: boolean;
  readonly #interactiveAppData: string | undefined;
  readonly #interactiveHome: string | undefined;
  readonly #runtimeDisposers: (() => void)[] = [];
  #dataLifecycle: DataLifecycleService | null = null;
  #diagnostics: DiagnosticLogger | null = null;
  #settings: SettingsStore | null = null;
  #history: HistoryStore | null = null;
  #vault: CredentialVault | null = null;
  #models: ModelManager | null = null;
  #whisper: WhisperWorkerClient | null = null;
  #providers: ProviderService | null = null;
  #providerMutations: ProviderMutationService | null = null;
  #providerOperations: ProviderOperationCoordinator | null = null;
  #updateOperations: UpdateOperationCoordinator | null = null;
  #applicationUpdates: ApplicationUpdateController | null = null;
  #windows: WindowManager | null = null;
  #tray: TrayController | null = null;
  #recording: RecordingService | null = null;
  #echo: EchoSessionController | null = null;
  #helper: HelperClient | null = null;
  #ipc: IpcTransportLifecycle | null = null;
  #removeHelperReadiness: (() => void) | null = null;
  #removeModelEvents: (() => void) | null = null;
  #startPromise: Promise<void> | null = null;
  #quitPromise: Promise<void> | null = null;
  #quitDeadline = 0;
  #boundedQuit: BoundedElectronQuit | null = null;
  #resetDeadline: number | null = null;
  #settingsFlush: Promise<void> | null = null;
  #vaultFlush: Promise<void> | null = null;
  #shutdownProgress: LifecycleProgress | null = null;
  #lifecycle: ApplicationLifecycle = 'new';
  #quitAllowed = false;
  #shutdownComplete = false;
  #processExitRequested = false;
  #resetRestartScheduled = false;
  #resetPending = false;
  #resetAcknowledgementToken: string | null = null;
  #skipDependentShutdown = false;
  #updateInstallRequested = false;
  #showMainWhenReady = false;
  #applicationActivationSequence = 0;
  readonly #pendingWindowsUpdateRelaunchGenerations = new Set<string>();
  readonly #inFlightWindowsUpdateRelaunchGenerations = new Set<string>();
  readonly #testQuitRequest = () => this.quit();

  constructor(options: TalkingQuillApplicationOptions = {}) {
    this.#windowsLoginStart = options.windowsLoginStart === true;
    this.#packagedEgressProof = options.packagedEgressProof === true;
    this.#interactiveAppData = options.interactiveAppData;
    this.#interactiveHome = options.interactiveHome;
  }

  start(): Promise<void> {
    if (this.#startPromise !== null) return this.#startPromise;
    if (this.#lifecycle !== 'new') return Promise.reject(new Error('Application cannot start'));
    this.#lifecycle = 'starting';
    this.#startPromise = this.#startInternal();
    return this.#startPromise;
  }

  stop(): Promise<void> {
    this.quit();
    return this.#quitPromise ?? Promise.resolve();
  }

  handleWindowsUpdateRelaunchGeneration(generation: string): void {
    if (!/^[0-9a-f]{32}$/.test(generation)) return;
    this.#pendingWindowsUpdateRelaunchGenerations.add(generation);
    if (this.#lifecycle === 'running') this.#acknowledgePendingWindowsUpdateRelaunches();
  }

  #acknowledgePendingWindowsUpdateRelaunches(): void {
    if (process.platform !== 'win32' || !app.isPackaged) return;
    const readyHelper = this.#helperExecutablePath();
    if (readyHelper === null) return;
    for (const generation of this.#pendingWindowsUpdateRelaunchGenerations) {
      if (this.#inFlightWindowsUpdateRelaunchGenerations.has(generation)) continue;
      this.#inFlightWindowsUpdateRelaunchGenerations.add(generation);
      void acknowledgeWindowsUpdateAppReady(
        readyHelper,
        app.getVersion(),
        generation,
        launchWindowsUpdateReadyHelper,
      )
        .then(() => this.#pendingWindowsUpdateRelaunchGenerations.delete(generation))
        .catch(() => undefined)
        .finally(() => this.#inFlightWindowsUpdateRelaunchGenerations.delete(generation));
    }
  }

  async #startInternal(): Promise<void> {
    const cleanup = new StartupCleanupStack();
    try {
      const sourceHarness = new SourceE2EHarness({
        isPackaged: app.isPackaged,
        environment: process.env,
        argv: process.argv,
      });
      const paths = createAppPaths(app.getPath('userData'));
      const helperExecutablePath = this.#helperExecutablePath();
      if (
        app.isPackaged &&
        process.platform === 'darwin' &&
        helperExecutablePath !== null &&
        validInstalledMacosOwner(process.resourcesPath, helperExecutablePath)
      ) {
        // This signed native mode validates pinned CMS cleanup journals and
        // finishes an interrupted committed uninstall before provisioning,
        // updater setup, or any owner connection can run.
        execFileSync(helperExecutablePath, ['--macos-owner-resume-cleanup'], {
          stdio: 'ignore',
          timeout: 30_000,
        });
      }
      const dataLifecycle = new DataLifecycleService(paths.root, {
        allowedBase: app.getPath('appData'),
        homeDirectory: app.getPath('home'),
        ...(helperExecutablePath === null
          ? {}
          : {
              removeIdentityBoundDirectory: createNativeOwnedTreeRemoval(helperExecutablePath),
            }),
      });
      this.#dataLifecycle = dataLifecycle;
      // Existing roots are rejected if they are links before recovery can touch descendants.
      // A missing root is created only after sibling-journal recovery has completed.
      const existingProfile = validateAppRootBeforeUse(paths, app.getPath('appData'), false);
      if (existingProfile) await dataLifecycle.reconcileCopiedProfile();
      await dataLifecycle.recoverPendingReset();
      validateAppRootBeforeUse(paths, app.getPath('appData'), true);
      await dataLifecycle.initializeOwnership();
      ensureAppDirectories(paths);
      const observeEgress = createEgressProofObserver(
        join(paths.temporary, 'egress-proof.jsonl'),
        egressProofRuntimeEnabled(this.#packagedEgressProof),
      );
      this.#assertStartupActive();

      const settings = new SettingsStore(paths.settingsFile, { migrations: SETTINGS_MIGRATIONS });
      this.#settings = settings;
      await settings.initialize();
      if (process.platform !== 'win32' && settings.get().recording.includeSystemAudio) {
        await settings.update({ recording: { includeSystemAudio: false } });
      }
      let diagnostics: DiagnosticLogger;
      try {
        diagnostics = new DiagnosticLogger(settings, paths.logs);
        this.#diagnostics = diagnostics;
        await diagnostics.initializeBestEffort();
      } catch (error: unknown) {
        await settings.flush();
        throw error;
      }
      // Rollback is LIFO. Flush durable user state before attempting diagnostic
      // disposal, which is reporting-only and must not hold settings hostage.
      cleanup.add('diagnostic-logger', () => diagnostics.disposeBestEffort());
      cleanup.add('settings', () => settings.flush());
      const launchAtLogin = new LaunchAtLoginService(app);
      cleanup.add('launch-at-login', () => launchAtLogin.dispose());
      try {
        launchAtLogin.reconcile(settings.get().app.launchAtLogin);
      } catch {
        // Do not claim registration when the OS did not confirm it.
        await settings.update({ app: { launchAtLogin: false } });
      }
      this.#assertStartupActive();

      const history = new HistoryStore(paths.historyDatabase);
      this.#history = history;
      cleanup.add('history', () => history.close());
      const commands = new VoiceCommandStore(settings);
      const vocabulary = new VocabularyStore(settings);
      const vocabularyDialogsLoader = sourceHarness.loadVocabularyDialogs(paths.root);
      const vocabularyDialogs =
        vocabularyDialogsLoader === undefined ? undefined : await vocabularyDialogsLoader;
      const vocabularyFiles = new VocabularyFileService(vocabulary, vocabularyDialogs);

      const vault = new CredentialVault(paths.credentialsFile, safeStorage);
      this.#vault = vault;
      cleanup.add('vault', () => vault.flush());
      await vault.initialize();
      this.#assertStartupActive();

      // Electron resolves these from the signed-in interactive user's Windows known folders even
      // when a packaged launch inherits a service-like PATH or incomplete environment.
      const resolvePiCli = sourceHarness.piResolverOverride();
      const providerRuntime = createProviderRuntime({
        settings,
        vault,
        workingDirectory: paths.root,
        observeEgress,
        platform: process.platform,
        ...(process.platform !== 'win32'
          ? {}
          : { interactiveAppData: this.#interactiveAppData ?? app.getPath('appData') }),
        ...(process.platform !== 'win32'
          ? {}
          : { interactiveHome: this.#interactiveHome ?? app.getPath('home') }),
        ...(resolvePiCli === undefined ? {} : { resolvePiCli }),
      });
      const { configs: providerConfigs, piInstallation, providers } = providerRuntime;
      this.#providers = providers;
      cleanup.add('provider-service', async () => {
        providers.dispose();
        await providers.drain();
      });
      const providerMutations = providerRuntime.createMutations();
      this.#providerMutations = providerMutations;
      cleanup.add('provider-mutations', async () => {
        providerMutations.stopAccepting();
        await providerMutations.drain();
      });
      await providerMutations.reconcileAll();
      this.#assertStartupActive();
      const providerOperations = new ProviderOperationCoordinator();
      this.#providerOperations = providerOperations;
      cleanup.add('provider-operations', () => providerOperations.dispose());
      const updateOperations = new UpdateOperationCoordinator();
      this.#updateOperations = updateOperations;
      cleanup.add('update-operations', () => updateOperations.dispose());

      const loader = new RendererLoader(
        selectDevelopmentRendererUrl(app.isPackaged, process.env.ELECTRON_RENDERER_URL),
      );
      const uiSession = session.fromPartition(UI_PARTITION);
      const captureSession = session.fromPartition(CAPTURE_PARTITION);
      if (loader.developmentOrigin === null) {
        const rendererRoot = join(__dirname, '..', 'renderer');
        this.#ownRuntimeDisposer(
          cleanup,
          'ui-protocol',
          installApplicationProtocol(rendererRoot, uiSession.protocol),
        );
        this.#ownRuntimeDisposer(
          cleanup,
          'capture-protocol',
          installApplicationProtocol(rendererRoot, captureSession.protocol, true),
        );
      }
      const microphonePermission = new MicrophonePermissionController();
      const systemAudioCapture = new SystemAudioCaptureController(captureSession);
      this.#ownRuntimeDisposer(cleanup, 'system-audio-capture', () => systemAudioCapture.dispose());
      this.#ownRuntimeDisposer(
        cleanup,
        'ui-session-policy',
        secureSession(uiSession, loader.developmentOrigin),
      );
      this.#ownRuntimeDisposer(
        cleanup,
        'capture-session-policy',
        secureSession(captureSession, loader.developmentOrigin, {
          allowWorkers: true,
          microphone: {
            controller: microphonePermission,
            getTrustedCaptureDocument: (webContents) =>
              getTrustedCaptureDocument(webContents, this.#roles),
          },
          systemAudio: {
            controller: systemAudioCapture,
            getTrustedCaptureDocument: (webContents) =>
              getTrustedCaptureDocument(webContents, this.#roles),
          },
        }),
      );
      this.#assertStartupActive();

      let windowTarget: WindowManager | null = null;
      const events = new IpcEventEmitter(this.#roles, () => windowTarget?.getWebContents() ?? []);
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
      this.#models = models;
      cleanup.add('models', () => models.shutdown());
      await models.initialize();
      this.#assertStartupActive();

      const whisper = new WhisperWorkerClient({
        cacheDirectory: paths.models,
        acquireModelUse: (modelId, signal) => models.acquireUse(modelId, signal),
      });
      this.#whisper = whisper;
      cleanup.add('whisper-worker', () => whisper.close());
      const modelRuntime = new ModelRuntimeCoordinator({ settings, events, models, whisper });
      const removeModelEvents = modelRuntime.subscribeProgress();
      this.#removeModelEvents = removeModelEvents;
      cleanup.add('model-events', removeModelEvents);

      const state = new AppStateService(settings, events);
      state.setModelReady((await modelRuntime.bindState(state)).state === 'ready');
      const recording = new RecordingService(
        new CaptureWindowClient(undefined, (id) => this.#roles.get(id)?.role ?? null),
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
          (process.platform === 'darwin' &&
            (process.arch === 'x64' || process.arch === 'arm64'))) &&
        existsSync(join(process.resourcesPath, 'app-update.yml'));
      const installedMacosOwnerAvailable =
        app.isPackaged &&
        process.platform === 'darwin' &&
        helperExecutablePath !== null &&
        validInstalledMacosOwner(process.resourcesPath, helperExecutablePath) &&
        existsSync(
          join(dirname(process.resourcesPath), 'MacOS', 'talking-quill-macos-service-bridge'),
        );
      const macosUpdateCoordinator = installedMacosOwnerAvailable
        ? new MacosOwnerUpdateCoordinator({
            helper: () => this.#helper,
            helperExecutable: helperExecutablePath,
            installedApp: dirname(dirname(process.resourcesPath)),
            temporaryRoot: app.getPath('temp'),
          })
        : null;
      const automaticInstallAvailable =
        automaticUpdateAvailable &&
        (process.platform !== 'darwin' || macosUpdateCoordinator !== null);
      const applicationUpdates = new ApplicationUpdateController({
        currentVersion: app.getVersion(),
        backend: automaticInstallAvailable ? createElectronUpdateBackend(process.arch) : null,
        ...(process.platform !== 'darwin' || macosUpdateCoordinator === null
          ? {}
          : { prepareInstall: (download) => macosUpdateCoordinator.prepareUpdate(download) }),
        publish: (update) => events.send('info:update-changed', update),
        requestInstall: () => this.#requestUpdateInstall(),
      });
      this.#applicationUpdates = applicationUpdates;
      this.#ownRuntimeDisposer(cleanup, 'application-updates', () => applicationUpdates.dispose());
      const systemInfo = new SystemInfoService(paths, () => microphonePermission.openSettings());
      const notices = new NoticesService(
        app.isPackaged
          ? join(process.resourcesPath, 'THIRD_PARTY_NOTICES.txt')
          : join(app.getAppPath(), 'assets', 'THIRD_PARTY_NOTICES.txt'),
      );
      this.#recording = recording;
      cleanup.add('recording', () => recording.shutdown());

      const helper = this.#createHelper(
        diagnostics.enabled ? join(paths.logs, 'owner-connection-helper-journal.json') : undefined,
      );
      if (helper === null) throw new Error('The native helper is unavailable on this platform');
      this.#helper = helper;
      cleanup.add('helper', () => this.#stopHelperAndDrainDiagnostics());
      this.#ownRuntimeDisposer(
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
      this.#removeHelperReadiness = removeHelperReadiness;

      const windows = new WindowManager(loader, this.#roles, settings, {
        requestQuit: () => this.quit(),
        onMaximizedChanged: (maximized) => events.send('window:maximized-changed', { maximized }),
        onMainHidden: () => {
          void recording.stopTest();
        },
        showMainOnFirstLoad:
          !this.#windowsLoginStart && !app.getLoginItemSettings().wasOpenedAtLogin,
      });
      this.#windows = windows;
      if (this.#showMainWhenReady) {
        this.#showMainWhenReady = false;
        windows.showMainByUser();
      }
      cleanup.add('windows', () => windows.destroyAll());
      windowTarget = windows;
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
      this.#echo = echo;
      this.#ownRuntimeDisposer(
        cleanup,
        'helper-wake-revalidation',
        installHelperWakeRevalidator({
          source: powerMonitor,
          isSafeToRevalidate: () =>
            this.#lifecycle === 'running' &&
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
      this.#ownRuntimeDisposer(cleanup, 'echo-state', removeEchoState);
      const removeSelectedModel = modelRuntime.subscribeSelectedModel(task6Composition !== null);
      this.#ownRuntimeDisposer(cleanup, 'selected-model', removeSelectedModel);

      const tray = new TrayController(state, {
        showMain: () => windows.showMainByUser(),
        quit: () => this.quit(),
        setEnabled: async (enabled) => {
          await echo.updateGeneral({ app: { enabled } });
        },
      });
      this.#tray = tray;
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
      this.#ownRuntimeDisposer(cleanup, 'tray-settings', removeTraySettings);

      const packagedMediaReady = sourceHarness.createPackagedMediaReady(task6Composition);
      const settingsTransferFiles = new SettingsTransferFileService(commands, echo);

      const ipc = registerIpcTransport(
        this.#roles,
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
          requestDataReset: () => this.#prepareDataReset(),
          acknowledgeDataReset: (token) => this.#acknowledgeDataReset(token),
        }),
      );
      this.#ipc = ipc;
      cleanup.add('ipc', async () => {
        ipc.stopAccepting();
        await ipc.drain();
        ipc.dispose();
      });

      // Every consumer and the hidden non-focusable widget exist before the native gateway can
      // enable activation. A fresh helper starts disabled, then Echo applies authoritative settings.
      await windows.createAll();
      this.#assertStartupActive();
      if (task6Composition === null) await helper.start();
      helperStartupComplete = true;
      this.#assertStartupActive();
      if (app.isPackaged) {
        void updates
          .check(app.getVersion(), this.#startupAbort.signal)
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
        this.#ownRuntimeDisposer(
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
        scavengeSessionArtifacts(paths.sessionTemporary, 64, this.#startupAbort.signal),
        historyService.pruneAtStartupDeferred(64, this.#startupAbort.signal),
      ]);
      this.#assertStartupActive();
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
        this.#assertStartupActive();
      }
      if (diagnostics.enabled) {
        await helper.getRuntimeObservability().catch(() => undefined);
      }
      this.#assertStartupActive();
      this.#lifecycle = 'running';
      this.#acknowledgePendingWindowsUpdateRelaunches();
      if (process.env.NODE_ENV === 'test') {
        Reflect.set(globalThis, '__talkingQuillRequestQuit', this.#testQuitRequest);
      }
      const localUpdate = process.argv.find((argument) =>
        argument.startsWith('--update-local-owner='),
      );
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
          this.quit();
          return;
        }
        if (localRollback !== undefined) {
          const archive = localRollback.slice('--rollback-local-owner='.length);
          await macosUpdateCoordinator.prepareRollback(localMacosUpdate(archive));
          this.quit();
          return;
        }
        if (process.argv.includes('--uninstall-local-owner')) {
          await macosUpdateCoordinator.prepareUninstall(() =>
            shell.trashItem(dirname(dirname(process.resourcesPath))),
          );
          this.quit();
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
      cleanup.disarm();
    } catch (error: unknown) {
      this.#lifecycle = this.#lifecycle === 'stopping' ? 'stopped' : 'failed';
      reportLifecycleDiagnostics(await cleanup.rollback(LIFECYCLE_TIMEOUT_MS));
      this.#clearOwnedReferences();
      throw error;
    }
  }

  showMain(): void {
    const windows = this.#windows;
    if (windows === null) {
      this.#showMainWhenReady = true;
      return;
    }
    windows.showMainByUser();
  }

  handleApplicationActivation(source: 'second_instance' | 'os_activate'): void {
    this.showMain();
    if (this.#applicationActivationSequence >= 8) return;
    this.#applicationActivationSequence += 1;
    void this.#diagnostics
      ?.record('application.activation', {
        component: 'application',
        outcome: 'requested',
        activationSource: source,
        activationSequence: this.#applicationActivationSequence,
        restoreHandlerReached: true,
        showMainReached: true,
      })
      .catch(() => undefined);
  }

  async #prepareDataReset(): Promise<string> {
    if (this.#lifecycle !== 'running' || this.#dataLifecycle === null || this.#resetPending) {
      throw new Error('Application data reset is unavailable');
    }
    // This synchronous gate runs before the first await. It removes every mutating IPC handler and
    // aborts provider/session work; only the typed, role-authorized one-time acknowledgement stays.
    this.#resetPending = true;
    const deadline = Date.now() + LIFECYCLE_TIMEOUT_MS;
    this.#resetDeadline = deadline;
    const acknowledgementToken = randomUUID();
    this.#resetAcknowledgementToken = acknowledgementToken;
    await prepareResetSafely({
      journal: this.#dataLifecycle,
      quiesce: () => this.#quiesce(true),
      criticalSteps: this.#createDrainSteps(['data:reset-all']),
      deadline,
      onAbort: (restartWithoutReset, abortDeadline) =>
        this.#abortAfterFailedReset(restartWithoutReset, abortDeadline),
    });
    if (!this.#resetRestartScheduled) {
      this.#resetRestartScheduled = true;
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
    this.#requestQuit({ deadline, skipDependentShutdown: true });
  }

  #acknowledgeDataReset(token: string): void {
    if (this.#resetAcknowledgementToken === null || token !== this.#resetAcknowledgementToken) {
      throw new Error('Reset acknowledgement is invalid or already consumed');
    }
    this.#resetAcknowledgementToken = null;
    this.#completeResetRelaunch();
  }

  #completeResetRelaunch(): void {
    if (
      this.#dataLifecycle?.resetPrepared !== true ||
      !this.#resetRestartScheduled ||
      this.#resetDeadline === null
    ) {
      return;
    }
    this.#resetRestartScheduled = false;
    this.#resetAcknowledgementToken = null;
    app.relaunch({ args: process.argv.slice(1) });
    this.#requestQuit({ deadline: this.#resetDeadline });
  }

  #requestUpdateInstall(): void {
    if (this.#lifecycle !== 'running' || this.#applicationUpdates === null) return;
    this.#updateInstallRequested = true;
    this.quit();
  }

  quit(deadline?: number): void {
    const effectiveDeadline = deadline ?? this.#resetDeadline;
    if (effectiveDeadline === null) this.#requestQuit();
    else this.#requestQuit({ deadline: effectiveDeadline });
  }

  #requestQuit(
    options: { readonly deadline?: number; readonly skipDependentShutdown?: boolean } = {},
  ): void {
    if (options.skipDependentShutdown === true) this.#skipDependentShutdown = true;
    if (this.#quitPromise !== null) return;
    this.#lifecycle = 'stopping';
    this.#quitDeadline = options.deadline ?? Date.now() + LIFECYCLE_TIMEOUT_MS;
    void this.#diagnostics
      ?.record('application.stopping', { component: 'application', outcome: 'requested' })
      .catch(() => undefined);
    this.#startupAbort.abort();
    this.#quiesce();
    // Start durable barriers before a broken renderer IPC, audio driver, or helper can consume the
    // remaining process deadline. The ordered drain below still awaits these same promises at the
    // normal settings and vault steps.
    this.#settingsFlush = this.#settings?.flush() ?? Promise.resolve();
    this.#vaultFlush = this.#vault?.flush() ?? Promise.resolve();
    void this.#settingsFlush.catch(() => undefined);
    void this.#vaultFlush.catch(() => undefined);
    this.#boundedQuit = createBoundedElectronQuit(app, this.#quitDeadline, {
      fallbackExitCode: 1,
      onDeadline: () => this.#forceQuitAtDeadline(),
    });
    this.#quitPromise = this.#drainBeforeQuit();
  }

  handleBeforeQuit(event: Electron.Event): void {
    if (this.#quitAllowed) {
      this.shutdown();
      return;
    }
    event.preventDefault();
    this.quit();
  }

  shutdown(): void {
    if (this.#shutdownComplete) return;
    this.#shutdownComplete = true;
    const runtimeDisposers = this.#runtimeDisposers.splice(0).reverse();
    const diagnostics = runSynchronousLifecycle('shutdown', [
      { name: 'quiesce', run: () => this.#quiesce() },
      { name: 'model-events', run: () => this.#removeModelEvents?.() },
      { name: 'helper-readiness', run: () => this.#removeHelperReadiness?.() },
      ...runtimeDisposers.map((dispose, index) => ({
        name: `runtime-disposer-${String(index + 1)}`,
        run: dispose,
      })),
      { name: 'ipc-dispose', run: () => this.#ipc?.dispose() },
      { name: 'tray', run: () => this.#tray?.destroy() },
      { name: 'windows', run: () => this.#windows?.destroyAll() },
      ...(this.#skipDependentShutdown
        ? []
        : [{ name: 'history', run: () => this.#history?.close() }]),
    ]);
    reportLifecycleDiagnostics(diagnostics);
    this.#clearOwnedReferences();
    if (Reflect.get(globalThis, '__talkingQuillRequestQuit') === this.#testQuitRequest) {
      Reflect.deleteProperty(globalThis, '__talkingQuillRequestQuit');
    }
    this.#lifecycle = 'stopped';
  }

  async #drainBeforeQuit(): Promise<void> {
    if (!(await this.#waitForStartupSettlement())) {
      // Startup cleanup is cancellation-aware, but an OS filesystem call can still stall. Never
      // let that hold the process open indefinitely or race asynchronous startup against teardown.
      this.#skipDependentShutdown = true;
      this.#finishQuit(1);
      return;
    }
    const diagnostics = await runBoundedLifecycle(
      'shutdown',
      this.#createDrainSteps(),
      LIFECYCLE_TIMEOUT_MS,
      {
        deadline: this.#quitDeadline,
        onProgress: (progress) => this.#observeShutdownProgress(progress),
      },
    );
    if (diagnostics.some(({ outcome }) => outcome === 'timed-out')) {
      this.#skipDependentShutdown = true;
    }
    reportLifecycleDiagnostics(diagnostics);
    this.#quitAllowed = true;
    if (this.#updateInstallRequested) {
      try {
        if (this.#applicationUpdates === null) throw new Error('Update installer is unavailable');
        this.#applicationUpdates.quitAndInstall();
        return;
      } catch {
        this.#updateInstallRequested = false;
      }
    }
    // All application-owned producers and durable stores have settled. Do not hand control back
    // to Chromium's graceful audio teardown, which can wait forever in a native driver.
    this.#finishQuit(diagnostics.some(({ outcome }) => outcome === 'timed-out') ? 1 : 0);
  }

  #observeShutdownProgress(progress: LifecycleProgress): void {
    this.#shutdownProgress = progress;
    if (process.env.NODE_ENV === 'test') {
      const snapshot = Object.freeze({ ...progress });
      Reflect.set(globalThis, '__talkingQuillShutdownProgress', snapshot);
      console.error('Talking Quill shutdown progress', snapshot);
    }
  }

  #forceQuitAtDeadline(): void {
    this.#skipDependentShutdown = true;
    console.error('Talking Quill shutdown deadline expired', {
      step: this.#shutdownProgress?.step ?? 'startup-settlement',
      pendingIpc: this.#ipc?.pendingChannels().slice(0, 16) ?? [],
    });
    this.shutdown();
  }

  #finishQuit(exitCode: number): void {
    if (this.#processExitRequested) return;
    this.#processExitRequested = true;
    this.#quitAllowed = true;
    this.#boundedQuit?.request(exitCode);
  }

  async #waitForStartupSettlement(): Promise<boolean> {
    const startup = this.#startPromise;
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
            Math.max(1, this.#quitDeadline - Date.now()),
          );
          timer.unref();
        }),
      ]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  #quiesce(preserveResetAcknowledgement = false): void {
    this.#windows?.beginQuit();
    this.#tray?.stopAccepting();
    this.#ipc?.stopAccepting(preserveResetAcknowledgement ? ['data:reset-renderer-ack'] : []);
    this.#providerMutations?.stopAccepting();
    this.#providerOperations?.dispose();
    this.#updateOperations?.dispose();
    this.#echo?.abort('shutdown');
    this.#providers?.dispose();
  }

  #createDrainSteps(excludedIpcChannels: readonly InvokeChannel[] = []): readonly LifecycleStep[] {
    return createApplicationDrainSteps(
      {
        ipc: this.#ipc,
        tray: this.#tray,
        providerMutations: this.#providerMutations,
        echo: this.#echo,
        providers: this.#providers,
        recording: this.#recording,
        models: this.#models,
        whisper: this.#whisper,
        helper:
          this.#helper === null ? null : { stop: () => this.#stopHelperAndDrainDiagnostics() },
        history: this.#history,
        settings:
          this.#settings === null
            ? null
            : {
                flush: () => settlePersistenceFlush(this.#settingsFlush, this.#settings?.flush()),
              },
        vault:
          this.#vault === null
            ? null
            : {
                flush: () => settlePersistenceFlush(this.#vaultFlush, this.#vault?.flush()),
              },
        diagnostics: this.#diagnostics,
      },
      excludedIpcChannels,
    );
  }

  async #stopHelperAndDrainDiagnostics(): Promise<void> {
    const failures: unknown[] = [];
    try {
      await this.#helper?.stop({ requireNeutral: true });
    } catch (error: unknown) {
      failures.push(error);
    }
    try {
      this.#removeHelperReadiness?.();
    } catch (error: unknown) {
      failures.push(error);
    } finally {
      this.#removeHelperReadiness = null;
    }
    // Diagnostic writes are best effort and may never settle. Persistence runs
    // before the diagnostic logger's independently bounded final disposal.
    if (failures.length > 0) throw failures[0];
  }

  #assertStartupActive(): void {
    if (this.#startupAbort.signal.aborted || this.#lifecycle !== 'starting') {
      throw new StartupCancelledError();
    }
  }

  #ownRuntimeDisposer(cleanup: StartupCleanupStack, name: string, dispose: () => void): void {
    let active = true;
    const ownedDispose = (): void => {
      if (!active) return;
      active = false;
      dispose();
    };
    this.#runtimeDisposers.push(ownedDispose);
    cleanup.add(name, ownedDispose);
  }

  #clearOwnedReferences(): void {
    this.#ipc = null;
    this.#removeModelEvents = null;
    this.#removeHelperReadiness = null;
    this.#helper = null;
    this.#recording = null;
    this.#echo = null;
    this.#tray = null;
    this.#windows = null;
    this.#history = null;
    this.#settings = null;
    this.#vault = null;
    this.#models = null;
    this.#whisper = null;
    this.#providers = null;
    this.#providerMutations = null;
    this.#providerOperations = null;
    this.#updateOperations = null;
    this.#applicationUpdates = null;
    this.#diagnostics = null;
    this.#dataLifecycle = null;
    this.#settingsFlush = null;
    this.#vaultFlush = null;
    this.#runtimeDisposers.length = 0;
  }

  #helperExecutablePath(): string | null {
    if (
      (process.platform !== 'win32' && process.platform !== 'darwin') ||
      (process.arch !== 'x64' && process.arch !== 'arm64')
    ) {
      return null;
    }
    return resolveHelperExecutable({
      packaged: app.isPackaged,
      resourcesPath: process.resourcesPath,
      appPath: app.getAppPath(),
      platform: process.platform,
    });
  }

  #createHelper(diagnosticJournalPath: string | undefined): HelperClient | null {
    const platform = process.platform;
    const architecture = process.arch;
    if (
      (platform !== 'win32' && platform !== 'darwin') ||
      (architecture !== 'x64' && architecture !== 'arm64')
    ) {
      return null;
    }
    const executablePath = this.#helperExecutablePath();
    if (executablePath === null) return null;
    return new HelperClient({
      executablePath,
      expectedHelperVersion: app.getVersion(),
      ...(diagnosticJournalPath === undefined ? {} : { diagnosticJournalPath }),
      platform,
      architecture,
      disableActivationCapture: activationCaptureRollbackEnabled(process.env),
      observeRuntimeObservability: (observability, source) => {
        void this.#diagnostics
          ?.record('helper.runtime.snapshot', {
            component: 'helper',
            outcome: source,
            observability,
          })
          .catch(() => undefined);
      },
      observeOwnerConnectionDiagnostic: (diagnostic) =>
        this.#diagnostics?.recordOwnerConnectionReplay(diagnostic) ?? Promise.resolve(false),
      observeProcessLifecycle: (event) => {
        void this.#diagnostics
          ?.record(event.phase === 'started' ? 'helper.process.started' : 'helper.process.exited', {
            component: 'helper',
            outcome:
              event.phase === 'started'
                ? 'runtime'
                : event.planned === true
                  ? 'shutdown'
                  : 'failure',
            runtimeVersion: app.getVersion(),
            ...(event.phase === 'exited'
              ? {
                  exitCode: event.exitCode ?? null,
                  exitSignal: event.signal ?? null,
                  planned: event.planned ?? false,
                }
              : {}),
          })
          .catch(() => undefined);
      },
    });
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

function localMacosUpdate(archive: string): DownloadedApplicationUpdate {
  if (process.arch !== 'x64' && process.arch !== 'arm64') {
    throw new Error('The current macOS architecture cannot validate local updater identity');
  }
  const identityPath = join(dirname(archive), `release-identity-mac-${process.arch}.json`);
  let value: unknown;
  try {
    value = JSON.parse(readFileSync(identityPath, 'utf8')) as unknown;
  } catch (error: unknown) {
    throw new Error('The local macOS update identity sidecar is missing or invalid', {
      cause: error,
    });
  }
  const version = (value as { readonly version?: unknown }).version;
  if (typeof version !== 'string') {
    throw new Error('The local macOS update identity has no release version');
  }
  return {
    files: [archive],
    identity: parseUnsignedUpdateIdentity(value, 'darwin', process.arch, version),
  };
}

function validInstalledMacosOwner(resourcesPath: string, helperExecutable: string): boolean {
  const marker = join(resourcesPath, 'keyboard-owner-installed-v1');
  const policy = join(resourcesPath, 'keyboard-owner-r5m.json');
  const bridge = join(dirname(resourcesPath), 'MacOS', 'talking-quill-macos-service-bridge');
  try {
    for (const path of [marker, policy, helperExecutable, bridge]) {
      const metadata = lstatSync(path);
      if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.nlink !== 1) return false;
    }
    const descriptor = openSync(marker, constants.O_RDONLY | constants.O_NOFOLLOW);
    let markerValue: string;
    try {
      markerValue = readFileSync(descriptor, 'utf8');
    } finally {
      closeSync(descriptor);
    }
    if (markerValue !== 'talking-quill-keyboard-owner-v1\n') return false;
    execFileSync(helperExecutable, ['--macos-owner-validate-install'], {
      stdio: 'ignore',
      timeout: 5_000,
      killSignal: 'SIGKILL',
    });
    return true;
  } catch {
    return false;
  }
}

function egressProofRuntimeEnabled(packagedProof: boolean): boolean {
  if (process.env.TALKING_QUILL_EGRESS_PROOF !== '1') return false;
  return app.isPackaged ? packagedProof : process.env.NODE_ENV === 'test';
}
