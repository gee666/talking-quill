import type { AppStateService } from '../app/app-state-service';
import type { LaunchAtLoginService } from '../app/launch-at-login-service';
import type { WindowManager } from '../app/window-manager';
import type { RecordingService } from '../audio/recording-service';
import type { EchoSessionController } from '../echo/echo-session-controller';
import type { HistoryService } from '../history/history-service';
import type { VoiceCommandStore } from '../commands/voice-command-store';
import type { VocabularyStore } from '../vocabulary/vocabulary-store';
import type { VocabularyFileService } from '../vocabulary/file-service';
import type { SettingsTransferFileService } from '../data/settings-transfer-file-service';
import type {
  ProviderConfigService,
  ProviderMutationService,
  ProviderOperationCoordinator,
  ProviderService,
  PiInstallationService,
} from '../providers';
import type { ModelManager } from '../transcription';
import type { SmartTranscriptionService } from '../smart/smart-transcription-service';
import type { WelcomeService } from '../welcome/welcome-service';
import type { UpdateService } from '../info/update-service';
import type { UpdateOperationCoordinator } from '../info/update-operation-coordinator';
import type { ApplicationUpdateController } from '../info/application-update-controller';
import type { SystemInfoService } from '../info/system-info-service';
import type { NoticesService } from '../info/notices-service';

export interface HandlerDependencies {
  readonly appVersion: string;
  readonly sourceRevision: string;
  readonly platform: string;
  readonly state: AppStateService;
  readonly launchAtLogin: LaunchAtLoginService;
  readonly providerConfigs: ProviderConfigService;
  readonly providerMutations: ProviderMutationService;
  readonly providerOperations: ProviderOperationCoordinator;
  readonly providers: ProviderService;
  readonly piInstallation: PiInstallationService;
  readonly smart: SmartTranscriptionService;
  readonly windows: WindowManager;
  readonly models: ModelManager;
  readonly recording: RecordingService;
  readonly echo: EchoSessionController;
  readonly history: HistoryService;
  readonly commands: VoiceCommandStore;
  readonly vocabulary: VocabularyStore;
  readonly vocabularyFiles: VocabularyFileService;
  readonly settingsTransferFiles: SettingsTransferFileService;
  readonly welcome: WelcomeService;
  readonly updates: UpdateService;
  readonly updateOperations: UpdateOperationCoordinator;
  readonly applicationUpdates: ApplicationUpdateController;
  readonly systemInfo: SystemInfoService;
  readonly notices: NoticesService;
  readonly diagnosticSummary: () => Readonly<Record<string, unknown>>;
  readonly packagedMediaReady?: (role: 'capture' | 'widget') => void;
  readonly requestDataReset: () => Promise<string>;
  readonly acknowledgeDataReset: (acknowledgementToken: string) => void;
}
