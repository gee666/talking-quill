import type { AppState } from '../schemas/app-state';
import type {
  ApplicationUpdateState,
  InfoLocation,
  InfoPermission,
  InfoStatus,
  UpdateCheckResult,
} from '../schemas/info';
import type { WelcomeState, WelcomeStep } from '../schemas/welcome';
import type { ActivationTestState } from '../schemas/activation-test';
import type { MicrophoneDeviceList, MicrophoneLevel, MicrophoneTestState } from '../schemas/audio';
import type {
  ProviderCredentialBindingToken,
  ProviderCredentialState,
} from '../schemas/credentials';
import type { EchoSessionSnapshot } from '../schemas/echo-session';
import type {
  HistoryCursor,
  HistoryDeleteAllResult,
  HistoryDeleteResult,
  HistoryPage,
} from '../schemas/history';
import type { WhisperModelId } from '../schemas/model-manifest';
import type { ModelDeleteResult, ModelProgress, ModelStatus } from '../schemas/transcription';
import { RunnableProviderIdSchema } from '../schemas/providers';
import type {
  Destination,
  ModelInfo,
  ProviderCatalogEntry,
  ProviderId,
  ProviderValidationResult,
  RunnableProviderConfig,
  RunnableProviderId,
  VisionCapability,
} from '../schemas/providers';
import type { PublicSettingsPatch, Settings } from '../schemas/settings';
import type {
  BuiltInDictationProfileId,
  CustomDictationProfileId,
  DictationProfileCreate,
  DictationProfileId,
  DictationProfilePatch,
} from '../schemas/dictation-profiles';
import type { PiInstallationStatus } from '../schemas/pi-installation';
import type {
  VoiceCommand,
  VoiceCommandInput,
  VoiceCommandMatch,
  VoiceCommandUpdate,
} from '../schemas/commands';
import type { VocabularyEntry, VocabularyFileResult } from '../schemas/vocabulary';

export interface BootstrapData {
  readonly appVersion: string;
  readonly sourceRevision?: string;
  readonly platform: string;
  readonly state: AppState;
  readonly settings: Settings;
}

export type Unsubscribe = () => void;

export interface MainApi {
  readonly welcome: {
    setStep(step: WelcomeStep): Promise<WelcomeState>;
    complete(): Promise<WelcomeState>;
  };
  readonly info: {
    status(): Promise<InfoStatus>;
    checkForUpdates(operationId: string): Promise<UpdateCheckResult>;
    cancel(operationId: string): Promise<boolean>;
    updateState(): Promise<ApplicationUpdateState>;
    applyUpdate(): Promise<ApplicationUpdateState>;
    onUpdateChanged(listener: (state: ApplicationUpdateState) => void): Unsubscribe;
    openPermissionSettings(permission: InfoPermission): Promise<void>;
    openLocation(location: InfoLocation): Promise<void>;
    openRelease(url: string): Promise<void>;
    notices(): Promise<string>;
  };
  readonly activationTest: {
    start(): Promise<ActivationTestState>;
    stop(): Promise<ActivationTestState>;
    onChanged(listener: (state: ActivationTestState) => void): Unsubscribe;
  };
  readonly shortcutCapture: {
    start(): Promise<void>;
    stop(): Promise<void>;
  };
  readonly app: {
    getBootstrap(): Promise<BootstrapData>;
    setEnabled(enabled: boolean): Promise<AppState>;
    onStateChanged(listener: (state: AppState) => void): Unsubscribe;
  };
  readonly settings: {
    update(patch: PublicSettingsPatch): Promise<Settings>;
    onChanged(listener: (settings: Settings) => void): Unsubscribe;
  };
  readonly profiles: {
    create(input: DictationProfileCreate): Promise<Settings>;
    update(id: DictationProfileId, patch: DictationProfilePatch): Promise<Settings>;
    delete(id: CustomDictationProfileId): Promise<Settings>;
    reset(id: BuiltInDictationProfileId): Promise<Settings>;
  };
  readonly data: {
    resetAll(confirmation: 'RESET TALKING QUILL'): Promise<void>;
    onResetAccepted(listener: () => void): Unsubscribe;
  };
  readonly recording: {
    getDevices(): Promise<MicrophoneDeviceList>;
    startTest(): Promise<MicrophoneTestState>;
    stopTest(): Promise<MicrophoneTestState>;
    openMicrophoneSettings(): Promise<void>;
    onDevicesChanged(listener: (devices: MicrophoneDeviceList) => void): Unsubscribe;
    onTestLevel(listener: (level: MicrophoneLevel) => void): Unsubscribe;
    onTestStateChanged(listener: (state: MicrophoneTestState) => void): Unsubscribe;
  };
  readonly echo: {
    onSessionChanged(listener: (snapshot: EchoSessionSnapshot) => void): Unsubscribe;
  };
  readonly history: {
    list(limit?: number, cursor?: HistoryCursor | null): Promise<HistoryPage>;
    delete(id: string): Promise<HistoryDeleteResult>;
    deleteAll(): Promise<HistoryDeleteAllResult>;
    copy(id: string): Promise<void>;
    thumbnail(id: string): Promise<string | null>;
    onChanged(listener: (revision: number) => void): Unsubscribe;
  };
  readonly commands: {
    list(): Promise<readonly VoiceCommand[]>;
    create(input: VoiceCommandInput): Promise<VoiceCommand>;
    update(id: string, patch: VoiceCommandUpdate): Promise<VoiceCommand>;
    delete(id: string): Promise<boolean>;
    preview(transcript: string): Promise<VoiceCommandMatch | null>;
  };
  readonly vocabulary: {
    list(): Promise<readonly VocabularyEntry[]>;
    create(value: string): Promise<VocabularyEntry>;
    update(id: string, value: string): Promise<VocabularyEntry>;
    delete(id: string): Promise<boolean>;
    importFile(): Promise<VocabularyFileResult>;
    exportFile(): Promise<VocabularyFileResult>;
  };
  readonly providers: {
    catalog(): Promise<readonly ProviderCatalogEntry[]>;
    piInstallationStatus(): Promise<PiInstallationStatus>;
    savePiInstallation(path: string | null): Promise<PiInstallationStatus>;
    browsePiInstallation(): Promise<string | null>;
    saveConfig(config: RunnableProviderConfig): Promise<{
      readonly settings: Settings;
      readonly credentialState: ProviderCredentialState;
    }>;
    setSecret(
      providerId: RunnableProviderId,
      expectedBindingToken: ProviderCredentialBindingToken,
      secret: string,
    ): Promise<ProviderCredentialState>;
    secretStatus(providerId: RunnableProviderId): Promise<ProviderCredentialState>;
    deleteSecret(
      providerId: RunnableProviderId,
      expectedBindingToken: ProviderCredentialBindingToken,
    ): Promise<ProviderCredentialState>;
    listModels(
      providerId: RunnableProviderId,
      operationId: string,
      refresh?: boolean,
    ): Promise<readonly ModelInfo[]>;
    testConnection(
      providerId: RunnableProviderId,
      operationId: string,
    ): Promise<ProviderValidationResult>;
    destination(providerId: RunnableProviderId, operationId: string): Promise<Destination>;
    cancel(operationId: string): Promise<boolean>;
    osaStatus(): Promise<{
      readonly providerId: RunnableProviderId;
      readonly modelId: string | null;
      readonly capability: VisionCapability;
      readonly manualTestAllowed: boolean;
      readonly screenPermission: 'granted' | 'denied' | 'unknown';
    }>;
    setOnScreenAwareness(enabled: boolean): Promise<Settings>;
    verifyVision(operationId: string, nonce: string): Promise<{ readonly verificationId: string }>;
    confirmVision(operationId: string, verificationId: string): Promise<Settings>;
  };
  readonly models: {
    list(): Promise<readonly ModelStatus[]>;
    status(modelId: WhisperModelId, verify?: boolean): Promise<ModelStatus>;
    download(modelId: WhisperModelId): Promise<ModelStatus>;
    pause(modelId: WhisperModelId): Promise<ModelStatus>;
    cancel(modelId: WhisperModelId): Promise<ModelStatus>;
    retry(modelId: WhisperModelId): Promise<ModelStatus>;
    delete(modelId: WhisperModelId): Promise<ModelDeleteResult>;
    onProgress(listener: (progress: ModelProgress) => void): Unsubscribe;
  };
  readonly windowControls: {
    minimize(): Promise<void>;
    toggleMaximize(): Promise<boolean>;
    close(): Promise<void>;
    onMaximizedChanged(listener: (maximized: boolean) => void): Unsubscribe;
  };
}

export interface WidgetApi {
  ready(): Promise<EchoSessionSnapshot>;
  stop(): Promise<void>;
  cancel(): Promise<void>;
  setInteractive(interactive: boolean): Promise<void>;
  onSessionChanged(listener: (snapshot: EchoSessionSnapshot) => void): Unsubscribe;
}

export interface CaptureApi {
  ready(): Promise<void>;
}

export function isRunnableProviderId(id: ProviderId): id is RunnableProviderId {
  return RunnableProviderIdSchema.safeParse(id).success;
}
