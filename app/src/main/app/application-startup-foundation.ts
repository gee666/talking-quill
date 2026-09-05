import { app, safeStorage, session } from 'electron';
import { join } from 'node:path';
import { CAPTURE_PARTITION, UI_PARTITION } from '../../shared/constants/app';
import { VoiceCommandStore } from '../commands/voice-command-store';
import { VocabularyStore } from '../vocabulary/vocabulary-store';
import { VocabularyFileService } from '../vocabulary/file-service';
import {
  CredentialVault,
  HistoryStore,
  SETTINGS_MIGRATIONS,
  SettingsStore,
  createAppPaths,
  ensureAppDirectories,
  validateAppRootBeforeUse,
} from '../persistence';
import { ProviderOperationCoordinator } from '../providers';
import { MicrophonePermissionController } from '../security/microphone-permission';
import { SystemAudioCaptureController } from '../security/system-audio-capture';
import { UpdateOperationCoordinator } from '../info/update-operation-coordinator';
import { DataLifecycleService } from '../data/data-lifecycle-service';
import { createNativeOwnedTreeRemoval } from '../data/native-owned-tree-removal';
import { DiagnosticLogger } from '../security/diagnostic-logger';
import { createEgressProofObserver } from '../security/egress-audit';
import { installApplicationProtocol } from '../security/protocol';
import { getTrustedCaptureDocument, secureSession } from '../security/session-policy';
import type { StartupCleanupStack } from './lifecycle';
import { LaunchAtLoginService } from './launch-at-login-service';
import { createProviderRuntime } from './provider-runtime';
import { RendererLoader, selectDevelopmentRendererUrl } from './renderer-loader';
import { SourceE2EHarness } from './source-e2e-harness';
import type { ApplicationRuntime } from './application-runtime';
import type { ApplicationStartupHooks } from './application-startup';

export async function prepareFoundation(
  runtime: ApplicationRuntime,
  cleanup: StartupCleanupStack,
  hooks: ApplicationStartupHooks,
) {
  const sourceHarness = new SourceE2EHarness({
    isPackaged: app.isPackaged,
    environment: process.env,
    argv: process.argv,
  });
  const paths = createAppPaths(app.getPath('userData'));
  const helperExecutablePath = hooks.helperExecutablePath();
  hooks.resumeMacosCleanup(helperExecutablePath);
  const dataLifecycle = new DataLifecycleService(paths.root, {
    allowedBase: app.getPath('appData'),
    homeDirectory: app.getPath('home'),
    ...(helperExecutablePath === null
      ? {}
      : {
          removeIdentityBoundDirectory: createNativeOwnedTreeRemoval(helperExecutablePath),
        }),
  });
  runtime.dataLifecycle = dataLifecycle;
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
    egressProofRuntimeEnabled(runtime.packagedEgressProof),
  );
  runtime.assertStartupActive();

  const settings = new SettingsStore(paths.settingsFile, { migrations: SETTINGS_MIGRATIONS });
  runtime.settings = settings;
  await settings.initialize();
  if (process.platform !== 'win32' && settings.get().recording.includeSystemAudio) {
    await settings.update({ recording: { includeSystemAudio: false } });
  }
  let diagnostics: DiagnosticLogger;
  try {
    diagnostics = new DiagnosticLogger(settings, paths.logs);
    runtime.diagnostics = diagnostics;
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
  runtime.assertStartupActive();

  const history = new HistoryStore(paths.historyDatabase);
  runtime.history = history;
  cleanup.add('history', () => history.close());
  const commands = new VoiceCommandStore(settings);
  const vocabulary = new VocabularyStore(settings);
  const vocabularyDialogsLoader = sourceHarness.loadVocabularyDialogs(paths.root);
  const vocabularyDialogs =
    vocabularyDialogsLoader === undefined ? undefined : await vocabularyDialogsLoader;
  const vocabularyFiles = new VocabularyFileService(vocabulary, vocabularyDialogs);

  const vault = new CredentialVault(paths.credentialsFile, safeStorage);
  runtime.vault = vault;
  cleanup.add('vault', () => vault.flush());
  await vault.initialize();
  runtime.assertStartupActive();

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
      : { interactiveAppData: runtime.interactiveAppData ?? app.getPath('appData') }),
    ...(process.platform !== 'win32'
      ? {}
      : { interactiveHome: runtime.interactiveHome ?? app.getPath('home') }),
    ...(resolvePiCli === undefined ? {} : { resolvePiCli }),
  });
  const { configs: providerConfigs, piInstallation, providers } = providerRuntime;
  runtime.providers = providers;
  cleanup.add('provider-service', async () => {
    providers.dispose();
    await providers.drain();
  });
  const providerMutations = providerRuntime.createMutations();
  runtime.providerMutations = providerMutations;
  cleanup.add('provider-mutations', async () => {
    providerMutations.stopAccepting();
    await providerMutations.drain();
  });
  await providerMutations.reconcileAll();
  runtime.assertStartupActive();
  const providerOperations = new ProviderOperationCoordinator();
  runtime.providerOperations = providerOperations;
  cleanup.add('provider-operations', () => providerOperations.dispose());
  const updateOperations = new UpdateOperationCoordinator();
  runtime.updateOperations = updateOperations;
  cleanup.add('update-operations', () => updateOperations.dispose());

  const loader = new RendererLoader(
    selectDevelopmentRendererUrl(app.isPackaged, process.env.ELECTRON_RENDERER_URL),
  );
  const uiSession = session.fromPartition(UI_PARTITION);
  const captureSession = session.fromPartition(CAPTURE_PARTITION);
  if (loader.developmentOrigin === null) {
    const rendererRoot = join(__dirname, '..', 'renderer');
    runtime.ownRuntimeDisposer(
      cleanup,
      'ui-protocol',
      installApplicationProtocol(rendererRoot, uiSession.protocol),
    );
    runtime.ownRuntimeDisposer(
      cleanup,
      'capture-protocol',
      installApplicationProtocol(rendererRoot, captureSession.protocol, true),
    );
  }
  const microphonePermission = new MicrophonePermissionController();
  const systemAudioCapture = new SystemAudioCaptureController(captureSession);
  runtime.ownRuntimeDisposer(cleanup, 'system-audio-capture', () => systemAudioCapture.dispose());
  runtime.ownRuntimeDisposer(
    cleanup,
    'ui-session-policy',
    secureSession(uiSession, loader.developmentOrigin),
  );
  runtime.ownRuntimeDisposer(
    cleanup,
    'capture-session-policy',
    secureSession(captureSession, loader.developmentOrigin, {
      allowWorkers: true,
      microphone: {
        controller: microphonePermission,
        getTrustedCaptureDocument: (webContents) =>
          getTrustedCaptureDocument(webContents, runtime.roles),
      },
      systemAudio: {
        controller: systemAudioCapture,
        getTrustedCaptureDocument: (webContents) =>
          getTrustedCaptureDocument(webContents, runtime.roles),
      },
    }),
  );
  runtime.assertStartupActive();

  return {
    sourceHarness,
    paths,
    helperExecutablePath,
    settings,
    diagnostics,
    launchAtLogin,
    history,
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
    microphonePermission,
    systemAudioCapture,
    observeEgress,
  };
}

export type StartupFoundation = Awaited<ReturnType<typeof prepareFoundation>>;

function egressProofRuntimeEnabled(packagedProof: boolean): boolean {
  if (process.env.TALKING_QUILL_EGRESS_PROOF !== '1') return false;
  return app.isPackaged ? packagedProof : process.env.NODE_ENV === 'test';
}
