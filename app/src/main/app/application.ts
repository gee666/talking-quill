import { app, clipboard, safeStorage, session, shell } from 'electron';

declare const __TALKING_QUILL_SOURCE_REVISION__: string;
import { randomUUID } from 'node:crypto';
import { writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION } from '../../shared/constants/app';
import type { InvokeChannel } from '../../shared/ipc/registry';
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
import { ProviderOperationCoordinator, PinnedJsonTransport } from '../providers';
import type { ProviderMutationService, ProviderService } from '../providers';
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
  type LifecycleStep,
  reportLifecycleDiagnostics,
  runBoundedLifecycle,
  runSynchronousLifecycle,
} from './lifecycle';
import { AppStateService } from './app-state-service';
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

const LIFECYCLE_TIMEOUT_MS = 5_000;
const RESET_ACKNOWLEDGEMENT_TIMEOUT_MS = 1_000;
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
      const sourceHarness = new SourceE2EHarness({
        isPackaged: app.isPackaged,
        environment: process.env,
        argv: process.argv,
      });
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
        environment: process.env,
        appData: app.getPath('appData'),
        home: app.getPath('home'),
        ...(resolvePiCli === undefined ? {} : { resolvePiCli }),
      });
      const { configs: providerConfigs, piInstallation, providers } = providerRuntime;
      this.#providers = providers;
      cleanup.add('provider-service', () => providers.dispose());
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
      const task6Loader = sourceHarness.loadTask6({ history, settings, recording });
      const task6Composition = task6Loader === null ? null : await task6Loader;
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
      const removeHelperReadiness = echoHelper.subscribeReadiness((readiness) => {
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
      cleanup.add('echo-model-readiness-target', modelRuntime.bindEcho(echo));
      cleanup.add('echo-session', () => echo.shutdown());
      const welcome = new WelcomeService(settings, {
        microphoneReady: () =>
          task6Composition?.welcome.microphone ?? recording.getState().permission === 'granted',
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
      recording.setWelcomeEvidenceInvalidator(() => {
        if (settings.get().welcome.microphoneEvidence != null) {
          void welcome.invalidateMicrophoneBinding();
        }
      });
      cleanup.add('welcome-readiness-target', removeModelWelcomeTarget);
      const removeEchoState = echo.subscribe((snapshot) => state.setSession(snapshot));
      this.#ownRuntimeDisposer(cleanup, 'echo-state', removeEchoState);
      if (task6Composition !== null) {
        this.#ownRuntimeDisposer(
          cleanup,
          'task6-test-driver',
          sourceHarness.bindAndExposeTask6(task6Composition, echo),
        );
      }

      const removeSelectedModel = modelRuntime.subscribeSelectedModel(task6Composition !== null);
      this.#ownRuntimeDisposer(cleanup, 'selected-model', removeSelectedModel);

      const tray = new TrayController(state, {
        showMain: () => windows.showMain(),
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
      criticalSteps: this.#createDrainSteps(['data:reset-all']),
      timeoutMs: LIFECYCLE_TIMEOUT_MS,
      onAbort: (restartWithoutReset) => this.#abortAfterFailedReset(restartWithoutReset),
    });
    if (!this.#resetRestartScheduled) {
      this.#resetRestartScheduled = true;
      // Bound the renderer paint/ack window. Relaunch is forced even if the renderer is hung.
      setTimeout(() => this.#completeResetRelaunch(), RESET_ACKNOWLEDGEMENT_TIMEOUT_MS);
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
      this.#createDrainSteps(),
      LIFECYCLE_TIMEOUT_MS,
    );
    if (diagnostics.some(({ outcome }) => outcome === 'timed-out')) {
      this.#skipDependentShutdown = true;
    }
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
    this.#tray?.stopAccepting();
    this.#ipc?.stopAccepting(preserveResetAcknowledgement ? ['data:reset-renderer-ack'] : []);
    this.#providerMutations?.stopAccepting();
    this.#providerOperations?.dispose();
    this.#updateOperations?.dispose();
    this.#providers?.dispose();
    this.#echo?.abort('shutdown');
  }

  #createDrainSteps(excludedIpcChannels: readonly InvokeChannel[] = []): readonly LifecycleStep[] {
    return createApplicationDrainSteps(
      {
        ipc: this.#ipc,
        tray: this.#tray,
        providerMutations: this.#providerMutations,
        echo: this.#echo,
        recording: this.#recording,
        models: this.#models,
        whisper: this.#whisper,
        helper: this.#helper,
        history: this.#history,
        settings: this.#settings,
        vault: this.#vault,
        diagnostics: this.#diagnostics,
      },
      excludedIpcChannels,
    );
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
