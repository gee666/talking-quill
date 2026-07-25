import { app, clipboard, safeStorage, session, shell } from 'electron';

declare const __TALKING_QUILL_TASK6_TEST_HARNESS__: boolean;
declare const __TALKING_QUILL_VOCABULARY_TEST_HARNESS__: boolean;
declare const __TALKING_QUILL_PI_TEST_HARNESS__: boolean;
declare const __TALKING_QUILL_SOURCE_REVISION__: string;
import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { isAbsolute, join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION } from '../../shared/constants/app';
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
import { HelperClient, resolveHelperExecutable } from '../helper';
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
import {
  ProviderConfigService,
  ProviderCredentialService,
  ProviderMutationService,
  ProviderOperationCoordinator,
  ProviderRegistry,
  PiInstallationService,
  PinnedJsonTransport,
  ProviderService,
} from '../providers';
import { MicrophonePermissionController } from '../security/microphone-permission';
import { WelcomeService } from '../welcome/welcome-service';
import { UpdateService } from '../info/update-service';
import { UpdateOperationCoordinator } from '../info/update-operation-coordinator';
import { SystemInfoService } from '../info/system-info-service';
import { NoticesService } from '../info/notices-service';
import { DataLifecycleService } from '../data/data-lifecycle-service';
import { createNativeOwnedTreeRemoval } from '../data/native-owned-tree-removal';
import { prepareResetSafely } from '../data/reset-preparation';
import { DiagnosticLogger } from '../security/diagnostic-logger';
import { createEgressProofObserver } from '../security/egress-audit';
import { installApplicationProtocol } from '../security/protocol';
import { getTrustedCaptureDocument, secureSession } from '../security/session-policy';
import {
  StartupCancelledError,
  StartupCleanupStack,
  reportLifecycleDiagnostics,
  runBoundedLifecycle,
  runSynchronousLifecycle,
} from './lifecycle';
import { AppStateService } from './app-state-service';
import { LaunchAtLoginService } from './launch-at-login-service';
import { applySelectedModelProgress } from './model-readiness-sync';
import { RendererLoader, selectDevelopmentRendererUrl } from './renderer-loader';
import { TrayController } from './tray-controller';
import { WindowManager } from './window-manager';
import { WindowRoleRegistry } from './window-role-registry';

const LIFECYCLE_TIMEOUT_MS = 5_000;
type ApplicationLifecycle = 'new' | 'starting' | 'running' | 'stopping' | 'stopped' | 'failed';

export class TalkingQuillApplication {
  readonly #roles = new WindowRoleRegistry();
  readonly #startupAbort = new AbortController();
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
  #lifecycle: ApplicationLifecycle = 'new';
  #quitAllowed = false;
  #shutdownComplete = false;
  #resetRestartScheduled = false;
  #resetPending = false;
  #resetAcknowledgementToken: string | null = null;
  #skipDependentShutdown = false;

  start(): Promise<void> {
    if (this.#startPromise !== null) return this.#startPromise;
    if (this.#lifecycle !== 'new') return Promise.reject(new Error('Application cannot start'));
    this.#lifecycle = 'starting';
    this.#startPromise = this.#startInternal();
    return this.#startPromise;
  }

  async #startInternal(): Promise<void> {
    const cleanup = new StartupCleanupStack();
    try {
      const paths = createAppPaths(app.getPath('userData'));
      const helperExecutablePath = this.#helperExecutablePath();
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
      validateAppRootBeforeUse(paths, app.getPath('appData'), false);
      await dataLifecycle.recoverPendingReset();
      validateAppRootBeforeUse(paths, app.getPath('appData'), true);
      await dataLifecycle.initializeOwnership();
      ensureAppDirectories(paths);
      const observeEgress = createEgressProofObserver(
        join(paths.temporary, 'egress-proof.jsonl'),
        egressProofRuntimeEnabled(),
      );
      this.#assertStartupActive();

      const settings = new SettingsStore(paths.settingsFile, { migrations: SETTINGS_MIGRATIONS });
      this.#settings = settings;
      cleanup.add('settings', () => settings.flush());
      await settings.initialize();
      const diagnostics = new DiagnosticLogger(settings, paths.logs);
      this.#diagnostics = diagnostics;
      cleanup.add('diagnostic-logger', () => diagnostics.dispose());
      await diagnostics.initialize().catch(() => undefined);
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
      const vocabularyDialogs =
        __TALKING_QUILL_VOCABULARY_TEST_HARNESS__ && sourceVocabularyRuntimeEnabled()
          ? await import('../vocabulary/source-test-dialogs').then(
              ({ createSourceTestVocabularyDialogs }) =>
                createSourceTestVocabularyDialogs(paths.root),
            )
          : undefined;
      const vocabularyFiles = new VocabularyFileService(vocabulary, vocabularyDialogs);

      const vault = new CredentialVault(paths.credentialsFile, safeStorage);
      this.#vault = vault;
      cleanup.add('vault', () => vault.flush());
      await vault.initialize();
      this.#assertStartupActive();

      const providerCredentials = new ProviderCredentialService(vault);
      const providerConfigs = new ProviderConfigService(settings);
      // Electron resolves this from the signed-in interactive user's Windows known folder even
      // when a packaged launch inherits a service-like PATH or incomplete environment.
      const packagedTestAppData = process.env.TALKING_QUILL_TEST_INTERACTIVE_APPDATA;
      const interactiveAppData =
        process.platform !== 'win32'
          ? undefined
          : process.env.TALKING_QUILL_PACKAGED_TEST === '1' &&
              packagedTestAppData !== undefined &&
              isAbsolute(packagedTestAppData)
            ? packagedTestAppData
            : app.getPath('appData');
      const packagedTestHome = process.env.TALKING_QUILL_TEST_INTERACTIVE_HOME;
      const interactiveHome =
        process.platform !== 'win32'
          ? undefined
          : process.env.TALKING_QUILL_PACKAGED_TEST === '1' &&
              packagedTestHome !== undefined &&
              isAbsolute(packagedTestHome)
            ? packagedTestHome
            : app.getPath('home');
      const piInstallation = new PiInstallationService(settings, {
        ...(interactiveAppData === undefined ? {} : { interactiveAppData }),
        ...(interactiveHome === undefined ? {} : { interactiveHome }),
      });
      const providers = new ProviderService(
        new ProviderRegistry({
          transport: new PinnedJsonTransport(undefined, {
            category: 'provider',
            observeEgress,
          }),
          pi: {
            observeEgress,
            workingDirectory: paths.root,
            configuredPath: () => piInstallation.configuredPath(),
            ...(interactiveAppData === undefined ? {} : { interactiveAppData }),
            ...(interactiveHome === undefined ? {} : { interactiveHome }),
            ...(__TALKING_QUILL_PI_TEST_HARNESS__ &&
            process.env.TALKING_QUILL_PI_TEST_UNAVAILABLE === '1'
              ? { resolveCli: () => Promise.reject(new Error('Pi unavailable test resolver')) }
              : {}),
          },
        }),
        providerCredentials,
      );
      this.#providers = providers;
      cleanup.add('provider-service', () => providers.dispose());
      const providerMutations = new ProviderMutationService(
        providerConfigs,
        providerCredentials,
        providers,
      );
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
        }),
      );
      this.#assertStartupActive();

      let windowTarget: WindowManager | null = null;
      const events = new IpcEventEmitter(this.#roles, () => windowTarget?.getWebContents() ?? []);
      const testNow = sourceTestNow();
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
      const runtimeValidatedModels = new Set<string>();
      const runtimeValidationTasks = new Map<string, Promise<boolean>>();
      const runtimeValidationKey = (modelId: Parameters<ModelManager['manifestRevision']>[0]) =>
        `${modelId}\u0000${models.manifestRevision(modelId)}`;
      models.setBeforeMutation(async (modelId) => {
        runtimeValidatedModels.delete(runtimeValidationKey(modelId));
        await whisper.unload(modelId);
      });
      models.setAfterInstallValidation(async (modelId, signal) => {
        await whisper.checkWorkerModel(modelId, signal);
        runtimeValidatedModels.add(runtimeValidationKey(modelId));
      });
      let stateTarget: AppStateService | null = null;
      let echoTarget: EchoSessionController | null = null;
      let welcomeTarget: WelcomeService | null = null;
      const removeModelEvents = models.subscribe((progress) => {
        events.send('model:progress', progress);
        if (progress.state !== 'ready') {
          runtimeValidatedModels.delete(runtimeValidationKey(progress.modelId));
        }
        const selectedModelId = settings.get().transcription.modelId;
        applySelectedModelProgress(progress, selectedModelId, stateTarget, echoTarget);
        if (
          progress.modelId === selectedModelId &&
          progress.state !== 'ready' &&
          settings.get().welcome.modelEvidence != null
        ) {
          void welcomeTarget?.invalidateModelSelection();
        }
      });
      this.#removeModelEvents = removeModelEvents;
      cleanup.add('model-events', removeModelEvents);

      const state = new AppStateService(settings, events);
      stateTarget = state;
      state.setModelReady(
        (await models.status(settings.get().transcription.modelId)).state === 'ready',
      );
      const recording = new RecordingService(
        new CaptureWindowClient(),
        settings,
        events,
        microphonePermission,
      );
      const updates = new UpdateService(
        new PinnedJsonTransport(undefined, { category: 'update', observeEgress }),
      );
      const systemInfo = new SystemInfoService(paths, () => microphonePermission.openSettings());
      const notices = new NoticesService(
        app.isPackaged
          ? join(process.resourcesPath, 'THIRD_PARTY_NOTICES.txt')
          : join(app.getAppPath(), 'assets', 'THIRD_PARTY_NOTICES.txt'),
      );
      this.#recording = recording;
      cleanup.add('recording', () => recording.shutdown());

      const helper = this.#createHelper();
      if (helper === null) throw new Error('The native helper is unavailable on this platform');
      this.#helper = helper;
      cleanup.add('helper', () => helper.stop());
      const task6Composition =
        __TALKING_QUILL_TASK6_TEST_HARNESS__ && sourceTask6RuntimeEnabled()
          ? await import('../../../../tests/e2e/support/task6-test-composition').then(
              ({ createTask6TestComposition }) =>
                createTask6TestComposition(
                  history,
                  settings,
                  process.argv.includes('--talking-quill-task6-real-media') ? recording : undefined,
                ),
            )
          : null;
      const helperStartPromise =
        task6Composition === null
          ? helper.start().finally(() => state.setHelperReadiness(helper.readiness))
          : Promise.resolve();
      // Observe an early rejection while windows and IPC are assembled. The authoritative await
      // below still fails startup after every acquired dependency has rollback ownership.
      void helperStartPromise.catch(() => undefined);
      const echoHelper = task6Composition?.helper ?? helper;
      if (task6Composition !== null) state.setModelReady(true);
      state.setHelperReadiness(echoHelper.readiness);
      let helperReadinessGeneration = 0;
      let previousHelperReadiness = echoHelper.readiness;
      const removeHelperReadiness = echoHelper.subscribeReadiness((readiness) => {
        if (readiness.status !== previousHelperReadiness.status) {
          helperReadinessGeneration += 1;
          if (settings.get().welcome.activationEvidence != null) {
            void welcomeTarget?.invalidateActivationBinding();
          }
        }
        previousHelperReadiness = readiness;
        state.setHelperReadiness(readiness);
      });
      this.#removeHelperReadiness = removeHelperReadiness;
      cleanup.add('helper-readiness', removeHelperReadiness);

      const windows = new WindowManager(loader, this.#roles, settings, {
        requestQuit: () => this.quit(),
        onMaximizedChanged: (maximized) => events.send('window:maximized-changed', { maximized }),
        onMainHidden: () => {
          void recording.stopTest();
        },
      });
      this.#windows = windows;
      cleanup.add('windows', () => windows.destroyAll());
      windowTarget = windows;
      let widgetCaptureExclusions = 0;
      let restoreWidgetAfterCapture = false;
      const screenshots = new ScreenshotService({
        setWidgetExcluded: async (excluded) => {
          if (excluded) {
            if (widgetCaptureExclusions === 0) {
              restoreWidgetAfterCapture = windows.isWidgetVisible();
              windows.hideWidget();
            }
            widgetCaptureExclusions += 1;
            return;
          }
          widgetCaptureExclusions = Math.max(0, widgetCaptureExclusions - 1);
          if (widgetCaptureExclusions > 0 || !restoreWidgetAfterCapture) return;
          restoreWidgetAfterCapture = false;
          const front = await echoHelper.getFrontApp().catch(() => null);
          windows.showWidget(settings.get().app.widgetSize, front?.windowBounds ?? null);
        },
      });
      const smart = new SmartTranscriptionService({
        settings,
        configs: providerConfigs,
        providers,
        screenshots,
        helper: echoHelper,
        screenshotsDirectory: paths.screenshots,
      });
      const echo = new EchoSessionController({
        settings,
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
      echoTarget = echo;
      cleanup.add('echo-model-readiness-target', () => {
        echoTarget = null;
      });
      cleanup.add('echo-session', () => echo.shutdown());
      const welcome = new WelcomeService(settings, {
        microphoneReady: () =>
          task6Composition?.welcome.microphone ?? recording.getState().permission === 'granted',
        microphoneObservation: () =>
          task6Composition?.welcome.microphone
            ? { boundDeviceId: 'source-e2e-microphone', observedRms: 0.2, sampleCount: 3_200 }
            : recording.microphoneTestObservation(),
        modelReady: async () => {
          if (task6Composition !== null) return task6Composition.welcome.model;
          const modelId = settings.get().transcription.modelId;
          const key = runtimeValidationKey(modelId);
          const metadata = await models.status(modelId);
          if (metadata.state !== 'ready') {
            runtimeValidatedModels.delete(key);
            return false;
          }
          if (runtimeValidatedModels.has(key)) return true;
          const existing = runtimeValidationTasks.get(key);
          if (existing !== undefined) return existing;
          const validation = models
            .status(modelId, true)
            .then((status) => {
              if (status.state === 'ready') runtimeValidatedModels.add(key);
              return status.state === 'ready';
            })
            .finally(() => runtimeValidationTasks.delete(key));
          runtimeValidationTasks.set(key, validation);
          return validation;
        },
        modelRevision: (modelId) => models.manifestRevision(modelId),
        helperReady: () =>
          task6Composition?.welcome.helper ?? state.getState().helper.status === 'ready',
        helperReadinessGeneration: () => helperReadinessGeneration,
        activationGestureRecognized: () => {
          const activation = echo.activationTestState;
          if (
            (activation.phase !== 'quick' && activation.phase !== 'extended') ||
            activation.profileId === null ||
            activation.activationKey === null
          ) {
            return null;
          }
          return {
            profileId: activation.profileId,
            activationKey: activation.activationKey,
            shift: activation.shift,
          };
        },
      });
      welcomeTarget = welcome;
      recording.setWelcomeEvidenceInvalidator(() => {
        if (settings.get().welcome.microphoneEvidence != null) {
          void welcome.invalidateMicrophoneBinding();
        }
      });
      cleanup.add('welcome-readiness-target', () => {
        welcomeTarget = null;
      });
      const removeEchoState = echo.subscribe((snapshot) => state.setSession(snapshot));
      this.#ownRuntimeDisposer(cleanup, 'echo-state', removeEchoState);
      if (task6Composition !== null) {
        task6Composition.bind(echo);
        const testDriverSymbol = Symbol.for('talking-quill:task6-test-driver');
        Reflect.set(globalThis, testDriverSymbol, task6Composition.driver);
        this.#ownRuntimeDisposer(cleanup, 'task6-test-driver', () => {
          Reflect.deleteProperty(globalThis, testDriverSymbol);
        });
      }

      let selectedModelId = settings.get().transcription.modelId;
      const removeSelectedModel = settings.subscribe((next) => {
        if (next.transcription.modelId === selectedModelId) return;
        selectedModelId = next.transcription.modelId;
        if (task6Composition !== null) {
          state.setModelReady(true);
          echo.readinessChanged();
          return;
        }
        void models.status(next.transcription.modelId).then((status) => {
          if (settings.get().transcription.modelId === status.modelId) {
            state.setModelReady(status.state === 'ready');
            echo.readinessChanged();
          }
        });
      });
      this.#ownRuntimeDisposer(cleanup, 'selected-model', removeSelectedModel);

      const tray = new TrayController(state, {
        showMain: () => windows.showMain(),
        quit: () => this.quit(),
        setEnabled: async (enabled) => {
          await echo.updateGeneral({ app: { enabled } });
        },
      });
      this.#tray = tray;
      cleanup.add('tray', () => tray.destroy());
      let trayEnabled = settings.get().app.enabled;
      const removeTraySettings = settings.subscribe((next) => {
        if (next.app.enabled === trayEnabled) return;
        trayEnabled = next.app.enabled;
        tray.refresh();
      });
      this.#ownRuntimeDisposer(cleanup, 'tray-settings', removeTraySettings);

      const packagedMediaReadyRoles = new Set<'capture' | 'widget'>();
      let packagedMediaActivated = false;
      const packagedMediaReady =
        task6Composition !== null &&
        app.isPackaged &&
        process.env.TALKING_QUILL_PACKAGED_MEDIA_HARNESS === '1' &&
        process.argv.includes('--talking-quill-task6-real-media')
          ? (role: 'capture' | 'widget'): void => {
              packagedMediaReadyRoles.add(role);
              if (packagedMediaReadyRoles.size < 2 || packagedMediaActivated) return;
              packagedMediaActivated = true;
              task6Composition.startPackagedMedia();
            }
          : undefined;

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
          welcome,
          updates,
          updateOperations,
          systemInfo,
          notices,
          ...(packagedMediaReady === undefined ? {} : { packagedMediaReady }),
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

      await windows.createAll();
      this.#assertStartupActive();
      // Native activation remains disabled until every eager renderer has loaded, so a startup
      // shortcut cannot begin a session whose preloaded widget or capture surface is unavailable.
      echo.initialize();
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
      await helperStartPromise;
      this.#assertStartupActive();
      this.#lifecycle = 'running';
      await diagnostics
        .record('application.started', { component: 'application', outcome: 'ready' })
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
    this.#windows?.showMain();
  }

  async #prepareDataReset(): Promise<string> {
    if (this.#lifecycle !== 'running' || this.#dataLifecycle === null || this.#resetPending) {
      throw new Error('Application data reset is unavailable');
    }
    // This synchronous gate runs before the first await. It removes every mutating IPC handler and
    // aborts provider/session work; only the typed, role-authorized one-time acknowledgement stays.
    this.#resetPending = true;
    const acknowledgementToken = randomUUID();
    this.#resetAcknowledgementToken = acknowledgementToken;
    await prepareResetSafely({
      journal: this.#dataLifecycle,
      quiesce: () => this.#quiesce(true),
      criticalSteps: [
        { name: 'ipc-drain', run: () => this.#ipc?.drain(['data:reset-all']) },
        { name: 'provider-mutations', run: () => this.#providerMutations?.drain() },
        { name: 'echo-session', run: () => this.#echo?.shutdown() },
        { name: 'recording', run: () => this.#recording?.shutdown() },
        { name: 'models', run: () => this.#models?.shutdown() },
        { name: 'whisper-worker', run: () => this.#whisper?.close() },
        { name: 'helper', run: () => this.#helper?.stop() },
        { name: 'history', run: () => this.#history?.close() },
        { name: 'settings', run: () => this.#settings?.flush() },
        { name: 'vault', run: () => this.#vault?.flush() },
        { name: 'diagnostic-logger', run: () => this.#diagnostics?.dispose() },
      ],
      timeoutMs: LIFECYCLE_TIMEOUT_MS,
      onAbort: (restartWithoutReset) => this.#abortAfterFailedReset(restartWithoutReset),
    });
    if (!this.#resetRestartScheduled) {
      this.#resetRestartScheduled = true;
      // Bound the renderer paint/ack window. Relaunch is forced even if the renderer is hung.
      setTimeout(() => this.#completeResetRelaunch(), 1_000);
    }
    return acknowledgementToken;
  }

  #abortAfterFailedReset(restartWithoutReset: boolean): void {
    // A timed-out producer may still be executing. Do not enter the ordinary shutdown sequence,
    // which would close dependent stores beneath it; process termination is the cancellation edge.
    this.#skipDependentShutdown = true;
    this.#quitAllowed = true;
    if (restartWithoutReset) app.relaunch({ args: process.argv.slice(1) });
    app.quit();
  }

  #acknowledgeDataReset(token: string): void {
    if (this.#resetAcknowledgementToken === null || token !== this.#resetAcknowledgementToken) {
      throw new Error('Reset acknowledgement is invalid or already consumed');
    }
    this.#resetAcknowledgementToken = null;
    this.#completeResetRelaunch();
  }

  #completeResetRelaunch(): void {
    if (this.#dataLifecycle?.resetPrepared !== true || !this.#resetRestartScheduled) return;
    this.#resetRestartScheduled = false;
    this.#resetAcknowledgementToken = null;
    app.relaunch({ args: process.argv.slice(1) });
    this.quit();
  }

  quit(): void {
    if (this.#quitPromise !== null) return;
    this.#lifecycle = 'stopping';
    void this.#diagnostics
      ?.record('application.stopping', { component: 'application', outcome: 'requested' })
      .catch(() => undefined);
    this.#startupAbort.abort();
    this.#quiesce();
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
    this.#lifecycle = 'stopped';
  }

  async #drainBeforeQuit(): Promise<void> {
    if (!(await this.#waitForStartupSettlement())) {
      // Startup cleanup is cancellation-aware, but an OS filesystem call can still stall. Never
      // let that hold the process open indefinitely or race asynchronous startup against teardown.
      this.#skipDependentShutdown = true;
      this.shutdown();
      app.exit(1);
      return;
    }
    const diagnostics = await runBoundedLifecycle(
      'shutdown',
      [
        { name: 'ipc-drain', run: () => this.#ipc?.drain() },
        { name: 'provider-mutations', run: () => this.#providerMutations?.drain() },
        { name: 'echo-session', run: () => this.#echo?.shutdown() },
        { name: 'recording', run: () => this.#recording?.shutdown() },
        { name: 'models', run: () => this.#models?.shutdown() },
        { name: 'whisper-worker', run: () => this.#whisper?.close() },
        { name: 'helper', run: () => this.#helper?.stop() },
        { name: 'history', run: () => this.#history?.close() },
        { name: 'settings', run: () => this.#settings?.flush() },
        { name: 'vault', run: () => this.#vault?.flush() },
        { name: 'diagnostic-logger', run: () => this.#diagnostics?.dispose() },
      ],
      LIFECYCLE_TIMEOUT_MS,
    );
    reportLifecycleDiagnostics(diagnostics);
    this.#quitAllowed = true;
    app.quit();
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
          timer = setTimeout(() => resolveWait(false), LIFECYCLE_TIMEOUT_MS);
          timer.unref();
        }),
      ]);
    } finally {
      if (timer !== undefined) clearTimeout(timer);
    }
  }

  #quiesce(preserveResetAcknowledgement = false): void {
    this.#windows?.beginQuit();
    this.#ipc?.stopAccepting(preserveResetAcknowledgement ? ['data:reset-renderer-ack'] : []);
    this.#providerMutations?.stopAccepting();
    this.#providerOperations?.dispose();
    this.#updateOperations?.dispose();
    this.#providers?.dispose();
    this.#echo?.abort('shutdown');
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
    this.#diagnostics = null;
    this.#dataLifecycle = null;
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

  #createHelper(): HelperClient | null {
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
      platform,
      architecture,
    });
  }
}

function egressProofRuntimeEnabled(): boolean {
  if (process.env.TALKING_QUILL_EGRESS_PROOF !== '1') return false;
  if (!app.isPackaged) return process.env.NODE_ENV === 'test';
  return (
    process.env.CI === 'true' &&
    process.env.TALKING_QUILL_PACKAGED_TEST === '1' &&
    process.argv.some((argument) => argument.startsWith('--remote-debugging-port='))
  );
}

function sourceVocabularyRuntimeEnabled(): boolean {
  return (
    !app.isPackaged &&
    process.env.NODE_ENV === 'test' &&
    process.argv.includes('--talking-quill-vocabulary-test')
  );
}

function sourceTask6RuntimeEnabled(): boolean {
  const requested =
    process.argv.includes('--talking-quill-task6-test') ||
    process.argv.includes('--talking-quill-task6-real-media');
  return (
    requested &&
    ((!app.isPackaged && process.env.NODE_ENV === 'test') ||
      (app.isPackaged && process.env.TALKING_QUILL_PACKAGED_MEDIA_HARNESS === '1'))
  );
}

function sourceTestNow(): (() => number) | null {
  if (!__TALKING_QUILL_TASK6_TEST_HARNESS__ || !sourceTask6RuntimeEnabled()) return null;
  const argument = process.argv.find((value) => value.startsWith('--talking-quill-test-now='));
  if (argument === undefined) return null;
  const value = Number(argument.slice('--talking-quill-test-now='.length));
  if (!Number.isSafeInteger(value) || value < 0) throw new Error('Invalid test clock');
  return () => value;
}
