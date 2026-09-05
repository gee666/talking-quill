import type { RecordingService } from '../audio/recording-service';
import type { EchoSessionController } from '../echo/echo-session-controller';
import type { HelperClient } from '../helper';
import { type IpcTransportLifecycle } from '../ipc/transport';
import type { ModelManager, WhisperWorkerClient } from '../transcription';
import type { CredentialVault, HistoryStore, SettingsStore } from '../persistence';
import type { ProviderOperationCoordinator } from '../providers';
import type { ProviderMutationService, ProviderService } from '../providers';
import type { UpdateOperationCoordinator } from '../info/update-operation-coordinator';
import type { ApplicationUpdateController } from '../info/application-update-controller';
import type { DataLifecycleService } from '../data/data-lifecycle-service';
import type { DiagnosticLogger } from '../security/diagnostic-logger';
import type { StartupCleanupStack } from './lifecycle';
import { StartupCancelledError, type LifecycleProgress } from './lifecycle';
import { type BoundedElectronQuit } from './electron-quit';
import type { TrayController } from './tray-controller';
import type { WindowManager } from './window-manager';
import type { WindowRoleRegistry } from './window-role-registry';

// Leave Pi RPC's 5.75 second retirement envelope intact after earlier producer drains.
export const LIFECYCLE_TIMEOUT_MS = 15_000;
export type ApplicationLifecycle =
  'new' | 'starting' | 'running' | 'stopping' | 'stopped' | 'failed';
export interface TalkingQuillApplicationOptions {
  readonly windowsLoginStart?: boolean;
  readonly packagedEgressProof?: boolean;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
}

// These facade fields retain their private names for the installed-acceptance build overlay.
export interface ApplicationResourceOwner {
  readonly getLifecycle: () => ApplicationLifecycle;
  readonly setLifecycle: (value: ApplicationLifecycle) => void;
  readonly getHelper: () => HelperClient | null;
  readonly setHelper: (value: HelperClient | null) => void;
  readonly getSettings: () => SettingsStore | null;
  readonly setSettings: (value: SettingsStore | null) => void;
  readonly getWindows: () => WindowManager | null;
  readonly setWindows: (value: WindowManager | null) => void;
  readonly getDiagnostics: () => DiagnosticLogger | null;
  readonly setDiagnostics: (value: DiagnosticLogger | null) => void;
}

// Tracks acquired resources and lifecycle state shared by startup and bounded shutdown.
export class ApplicationRuntime {
  readonly #owner: ApplicationResourceOwner;
  readonly roles: WindowRoleRegistry;
  readonly startupAbort = new AbortController();
  readonly windowsLoginStart: boolean;
  readonly packagedEgressProof: boolean;
  readonly interactiveAppData: string | undefined;
  readonly interactiveHome: string | undefined;
  readonly runtimeDisposers: (() => void)[] = [];
  dataLifecycle: DataLifecycleService | null = null;
  get diagnostics(): DiagnosticLogger | null {
    return this.#owner.getDiagnostics();
  }
  set diagnostics(value: DiagnosticLogger | null) {
    this.#owner.setDiagnostics(value);
  }
  get settings(): SettingsStore | null {
    return this.#owner.getSettings();
  }
  set settings(value: SettingsStore | null) {
    this.#owner.setSettings(value);
  }
  history: HistoryStore | null = null;
  vault: CredentialVault | null = null;
  models: ModelManager | null = null;
  whisper: WhisperWorkerClient | null = null;
  providers: ProviderService | null = null;
  providerMutations: ProviderMutationService | null = null;
  providerOperations: ProviderOperationCoordinator | null = null;
  updateOperations: UpdateOperationCoordinator | null = null;
  applicationUpdates: ApplicationUpdateController | null = null;
  get windows(): WindowManager | null {
    return this.#owner.getWindows();
  }
  set windows(value: WindowManager | null) {
    this.#owner.setWindows(value);
  }
  tray: TrayController | null = null;
  recording: RecordingService | null = null;
  echo: EchoSessionController | null = null;
  get helper(): HelperClient | null {
    return this.#owner.getHelper();
  }
  set helper(value: HelperClient | null) {
    this.#owner.setHelper(value);
  }
  ipc: IpcTransportLifecycle | null = null;
  removeHelperReadiness: (() => void) | null = null;
  removeModelEvents: (() => void) | null = null;
  startPromise: Promise<void> | null = null;
  quitPromise: Promise<void> | null = null;
  quitDeadline = 0;
  boundedQuit: BoundedElectronQuit | null = null;
  resetDeadline: number | null = null;
  settingsFlush: Promise<void> | null = null;
  vaultFlush: Promise<void> | null = null;
  shutdownProgress: LifecycleProgress | null = null;
  get lifecycle(): ApplicationLifecycle {
    return this.#owner.getLifecycle();
  }
  set lifecycle(value: ApplicationLifecycle) {
    this.#owner.setLifecycle(value);
  }
  quitAllowed = false;
  shutdownComplete = false;
  processExitRequested = false;
  resetRestartScheduled = false;
  resetPending = false;
  resetAcknowledgementToken: string | null = null;
  skipDependentShutdown = false;
  updateInstallRequested = false;
  showMainWhenReady = false;
  readonly pendingWindowsUpdateRelaunchGenerations = new Set<string>();
  readonly inFlightWindowsUpdateRelaunchGenerations = new Set<string>();

  constructor(
    options: TalkingQuillApplicationOptions,
    roles: WindowRoleRegistry,
    owner: ApplicationResourceOwner,
  ) {
    this.#owner = owner;
    this.roles = roles;
    this.windowsLoginStart = options.windowsLoginStart === true;
    this.packagedEgressProof = options.packagedEgressProof === true;
    this.interactiveAppData = options.interactiveAppData;
    this.interactiveHome = options.interactiveHome;
  }

  assertStartupActive(): void {
    if (this.startupAbort.signal.aborted || this.lifecycle !== 'starting') {
      throw new StartupCancelledError();
    }
  }

  ownRuntimeDisposer(cleanup: StartupCleanupStack, name: string, dispose: () => void): void {
    let active = true;
    const ownedDispose = (): void => {
      if (!active) return;
      active = false;
      dispose();
    };
    this.runtimeDisposers.push(ownedDispose);
    cleanup.add(name, ownedDispose);
  }

  clearOwnedReferences(): void {
    this.ipc = null;
    this.removeModelEvents = null;
    this.removeHelperReadiness = null;
    this.helper = null;
    this.recording = null;
    this.echo = null;
    this.tray = null;
    this.windows = null;
    this.history = null;
    this.settings = null;
    this.vault = null;
    this.models = null;
    this.whisper = null;
    this.providers = null;
    this.providerMutations = null;
    this.providerOperations = null;
    this.updateOperations = null;
    this.applicationUpdates = null;
    this.diagnostics = null;
    this.dataLifecycle = null;
    this.settingsFlush = null;
    this.vaultFlush = null;
    this.runtimeDisposers.length = 0;
  }
}
