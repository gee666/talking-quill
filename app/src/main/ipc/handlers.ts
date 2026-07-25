import type { AppStateService } from '../app/app-state-service';
import type { LaunchAtLoginService } from '../app/launch-at-login-service';
import type { WindowManager } from '../app/window-manager';
import type { RecordingService } from '../audio/recording-service';
import type { EchoSessionController } from '../echo/echo-session-controller';
import type { HistoryService } from '../history/history-service';
import type { VoiceCommandStore } from '../commands/voice-command-store';
import type { VocabularyStore } from '../vocabulary/vocabulary-store';
import type { VocabularyFileService } from '../vocabulary/file-service';
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
import type { SystemInfoService } from '../info/system-info-service';
import type { NoticesService } from '../info/notices-service';
import type { InvokeHandlerMap } from './types';

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
  readonly welcome: WelcomeService;
  readonly updates: UpdateService;
  readonly updateOperations: UpdateOperationCoordinator;
  readonly systemInfo: SystemInfoService;
  readonly notices: NoticesService;
  readonly packagedMediaReady?: (role: 'capture' | 'widget') => void;
  readonly requestDataReset: () => Promise<string>;
  readonly acknowledgeDataReset: (acknowledgementToken: string) => void;
}

export function createHandlers(dependencies: HandlerDependencies): InvokeHandlerMap {
  return {
    'bootstrap:get': () => ({
      appVersion: dependencies.appVersion,
      sourceRevision: dependencies.sourceRevision,
      platform: dependencies.platform,
      state: dependencies.state.getState(),
      settings: dependencies.state.getSettings(),
    }),
    'welcome:set-step': ({ step }) => dependencies.welcome.setStep(step),
    'welcome:complete': () => dependencies.welcome.complete(),
    'info:status': async () => ({
      microphone: (await dependencies.recording.getDevices()).permission,
      screenRecording: dependencies.smart.status().screenPermission,
      helper: dependencies.state.getState().helper,
    }),
    'info:check-update': ({ operationId }, context) =>
      dependencies.updateOperations.run(context, operationId, (signal) =>
        dependencies.updates.check(dependencies.appVersion, signal),
      ),
    'info:cancel-update': ({ operationId }, context) => ({
      cancelled: dependencies.updateOperations.cancel(context.webContentsId, operationId),
    }),
    'info:open-permission': async ({ permission }) => {
      await dependencies.systemInfo.openPermission(permission);
      return { accepted: true };
    },
    'info:open-location': async ({ location }) => {
      await dependencies.systemInfo.openLocation(location);
      return { accepted: true };
    },
    'info:open-release': async ({ url }) => {
      await dependencies.systemInfo.openRelease(url);
      return { accepted: true };
    },
    'info:notices': async () => ({ text: await dependencies.notices.read() }),
    'activation-test:start': (_request, context) =>
      dependencies.echo.startActivationTest(context.webContentsId, context.onDestroyed),
    'activation-test:stop': (_request, context) =>
      dependencies.echo.stopActivationTest(context.webContentsId),
    'app:set-enabled': async ({ enabled }) => {
      await dependencies.echo.updateGeneral({ app: { enabled } });
      return dependencies.state.getState();
    },
    'data:reset-all': async () => ({
      accepted: true,
      acknowledgementToken: await dependencies.requestDataReset(),
    }),
    'data:reset-renderer-ack': ({ acknowledgementToken }) => {
      dependencies.acknowledgeDataReset(acknowledgementToken);
      return { accepted: true };
    },
    'settings:update': async (patch) => {
      const before = dependencies.state.getSettings();
      if (
        patch.app?.launchAtLogin !== undefined &&
        patch.app.launchAtLogin !== before.app.launchAtLogin
      ) {
        dependencies.launchAtLogin.set(patch.app.launchAtLogin);
      }
      if (patch.app?.enabled !== undefined) {
        await dependencies.echo.updateGeneral(patch);
      } else {
        await dependencies.state.updateSettings(patch);
      }
      if (
        patch.recording?.preferredMicrophoneId !== undefined &&
        patch.recording.preferredMicrophoneId !== before.recording.preferredMicrophoneId
      ) {
        await dependencies.welcome.invalidateMicrophoneBinding();
      }
      if (
        patch.transcription?.modelId !== undefined &&
        patch.transcription.modelId !== before.transcription.modelId
      ) {
        await dependencies.welcome.invalidateModelSelection();
      }
      if (patch.app?.enabled !== undefined && patch.app.enabled !== before.app.enabled) {
        await dependencies.welcome.invalidateActivationBinding();
      }
      return dependencies.state.getSettings();
    },
    'profile:create': (input) => dependencies.echo.createProfile(input),
    'profile:update': ({ id, patch }) => dependencies.echo.updateProfile(id, patch),
    'profile:delete': ({ id }) => dependencies.echo.deleteProfile(id),
    'profile:reset': ({ id }) => dependencies.echo.resetProfile(id),
    'provider:catalog': () => ({ providers: [...dependencies.providers.catalog()] }),
    'provider:pi-installation-status': () => dependencies.piInstallation.status(),
    'provider:pi-installation-save': ({ path }) => dependencies.piInstallation.save(path),
    'provider:pi-installation-browse': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Pi installation dialog owner is unavailable');
      return dependencies.piInstallation.browse(owner);
    },
    'provider:config-save': ({ config }) =>
      dependencies.providerMutations.saveConfigWithCredentialState(config),
    'provider:secret-set': ({ providerId, expectedBindingToken, secret }) =>
      dependencies.providerMutations.setSecret(providerId, expectedBindingToken, secret),
    'provider:secret-status': ({ providerId }) =>
      dependencies.providerMutations.secretStatus(providerId),
    'provider:secret-delete': ({ providerId, expectedBindingToken }) =>
      dependencies.providerMutations.deleteSecret(providerId, expectedBindingToken),
    'provider:list-models': ({ providerId, operationId, refresh }, context) =>
      dependencies.providerOperations.run(context, operationId, async (signal) => ({
        providerId,
        models: [
          ...(await dependencies.providers.listModels(
            dependencies.providerConfigs.get(providerId),
            signal,
            { refresh },
          )),
        ],
      })),
    'provider:test-connection': ({ providerId, operationId }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.providers.testConnection(dependencies.providerConfigs.get(providerId), signal),
      ),
    'provider:destination': ({ providerId, operationId }, context) =>
      dependencies.providerOperations.run(context, operationId, async (signal) => ({
        destination: await dependencies.providers.classifyDestination(
          dependencies.providerConfigs.get(providerId),
          signal,
        ),
      })),
    'provider:cancel': ({ operationId }, context) => ({
      cancelled: dependencies.providerOperations.cancel(context.webContentsId, operationId),
    }),
    'provider:osa-status': () => dependencies.smart.status(),
    'provider:osa-set': ({ enabled }) => dependencies.smart.setOnScreenAwareness(enabled),
    'provider:vision-test': ({ operationId, nonce }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.smart.verifyManualVision(nonce, signal),
      ),
    'provider:vision-confirm': ({ operationId, verificationId }, context) =>
      dependencies.providerOperations.run(context, operationId, (signal) =>
        dependencies.smart.confirmManualVision(verificationId, signal),
      ),
    'history:list': (request) => dependencies.history.list(request),
    'history:delete': ({ id }) => dependencies.history.deleteById(id),
    'history:delete-all': () => dependencies.history.deleteAll(),
    'history:copy': ({ id }) => dependencies.history.copy(id),
    'history:thumbnail': ({ id }) => dependencies.history.thumbnail(id),
    'model:list': () => dependencies.models.list(),
    'model:status': ({ modelId, verify }) => dependencies.models.status(modelId, verify),
    'model:download': ({ modelId }) => dependencies.models.download(modelId),
    'model:pause': ({ modelId }) => dependencies.models.pause(modelId),
    'model:cancel': ({ modelId }) => dependencies.models.cancel(modelId),
    'model:retry': ({ modelId }) => dependencies.models.retry(modelId),
    'model:delete': ({ modelId }) => dependencies.models.deleteIfIdle(modelId),
    'window:minimize': (_request, context) => {
      dependencies.windows.getByWebContentsId(context.webContentsId)?.minimize();
      return { accepted: true };
    },
    'window:toggle-maximize': (_request, context) => {
      const window = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (window?.isMaximized()) window.unmaximize();
      else window?.maximize();
      return { maximized: window?.isMaximized() ?? false };
    },
    'window:close': async (_request, context) => {
      await dependencies.windows.closeByWebContentsId(context.webContentsId);
      return { accepted: true };
    },
    'widget:ready': () => {
      dependencies.packagedMediaReady?.('widget');
      return dependencies.echo.snapshot;
    },
    'widget:stop': () => {
      dependencies.echo.stop();
      return { accepted: true };
    },
    'widget:cancel': () => {
      dependencies.echo.cancel();
      return { accepted: true };
    },
    'widget:set-interactive': ({ interactive }, context) => {
      dependencies.windows.setWidgetInteractive(context.webContentsId, interactive);
      return { accepted: true };
    },
    'capture:ready': (_request, context) => {
      const window = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (window !== null) dependencies.recording.attachCapture(window.webContents);
      dependencies.packagedMediaReady?.('capture');
      return { accepted: true };
    },
    'recording:get-devices': () => dependencies.recording.getDevices(),
    'recording:start-test': async (_request, context) => {
      const result = await dependencies.recording.startTest(
        dependencies.windows.getByWebContentsId(context.webContentsId)?.webContents ?? null,
      );
      return result;
    },
    'recording:stop-test': (_request, context) =>
      dependencies.recording.stopTest(context.webContentsId),
    'recording:open-microphone-settings': async () => {
      await dependencies.recording.openMicrophoneSettings();
      return { accepted: true };
    },
    'commands:list': () => [...dependencies.commands.list()],
    'commands:create': (input) => dependencies.commands.create(input),
    'commands:update': ({ id, patch }) => dependencies.commands.update(id, patch),
    'commands:delete': async ({ id }) => ({ deleted: await dependencies.commands.delete(id) }),
    'commands:preview': ({ transcript }) => dependencies.commands.match(transcript),
    'vocabulary:list': () => [...dependencies.vocabulary.list()],
    'vocabulary:create': ({ value }) => dependencies.vocabulary.create(value),
    'vocabulary:update': ({ id, value }) => dependencies.vocabulary.update(id, value),
    'vocabulary:delete': async ({ id }) => ({ deleted: await dependencies.vocabulary.delete(id) }),
    'vocabulary:import-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Vocabulary dialog owner is unavailable');
      return dependencies.vocabularyFiles.importFile(owner);
    },
    'vocabulary:export-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Vocabulary dialog owner is unavailable');
      return dependencies.vocabularyFiles.exportFile(owner);
    },
  };
}
