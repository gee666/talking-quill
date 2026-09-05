import type { HandlerDependencies } from './handler-dependencies';
import type { InvokeHandlerMap } from './types';
import type { PublicSettingsPatch, Settings } from '../../shared/schemas/settings';

type SettingsHandlerDependencies = Pick<
  HandlerDependencies,
  'echo' | 'launchAtLogin' | 'platform' | 'settingsTransferFiles' | 'state' | 'welcome' | 'windows'
>;
type SettingsHandlers = Pick<
  InvokeHandlerMap,
  | 'app:set-enabled'
  | 'settings:update'
  | 'profile:create'
  | 'profile:update'
  | 'profile:delete'
  | 'profile:reset'
  | 'profile:import-file'
  | 'profile:export-file'
>;

// Settings and profiles share this queue for the lifetime of one handler map.
export function createSettingsHandlers(
  dependencies: SettingsHandlerDependencies,
): SettingsHandlers {
  let settingsMutationQueue: Promise<void> = Promise.resolve();
  const serializeSettingsMutation = <Result>(operation: () => Promise<Result>): Promise<Result> => {
    const mutation = settingsMutationQueue.then(operation);
    settingsMutationQueue = mutation.then(
      () => undefined,
      () => undefined,
    );
    return mutation;
  };

  return {
    'app:set-enabled': ({ enabled }) =>
      serializeSettingsMutation(async () => {
        await updateSettings(dependencies, { app: { enabled } });
        return dependencies.state.getState();
      }),
    'settings:update': (patch) =>
      serializeSettingsMutation(() => updateSettings(dependencies, patch)),
    'profile:create': (input) =>
      serializeSettingsMutation(() => dependencies.echo.createProfile(input)),
    'profile:update': ({ id, patch }) =>
      serializeSettingsMutation(() => dependencies.echo.updateProfile(id, patch)),
    'profile:delete': ({ id }) =>
      serializeSettingsMutation(() => dependencies.echo.deleteProfile(id)),
    'profile:reset': ({ id }) =>
      serializeSettingsMutation(() => dependencies.echo.resetProfile(id)),
    'profile:import-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Dictation profile dialog owner is unavailable');
      return serializeSettingsMutation(() =>
        dependencies.settingsTransferFiles.importDictationProfiles(owner),
      );
    },
    'profile:export-file': (_request, context) => {
      const owner = dependencies.windows.getByWebContentsId(context.webContentsId);
      if (owner === null) throw new Error('Dictation profile dialog owner is unavailable');
      return dependencies.settingsTransferFiles.exportDictationProfiles(owner);
    },
  };
}

async function updateSettings(
  dependencies: SettingsHandlerDependencies,
  patch: PublicSettingsPatch,
): Promise<Settings> {
  const before = dependencies.state.getSettings();
  const microphoneChanged =
    patch.recording?.preferredMicrophoneId !== undefined &&
    patch.recording.preferredMicrophoneId !== before.recording.preferredMicrophoneId;
  const modelChanged =
    patch.transcription?.modelId !== undefined &&
    patch.transcription.modelId !== before.transcription.modelId;
  const requestedLaunchAtLogin = patch.app?.launchAtLogin;
  if (patch.recording?.includeSystemAudio === true && dependencies.platform !== 'win32') {
    throw new Error('System audio capture is unavailable on this platform');
  }

  // Evidence is derived from the values being replaced. Clear it before committing so a failed
  // invalidation can be retried with the same patch instead of becoming invisible after commit.
  if (microphoneChanged) await dependencies.welcome.invalidateMicrophoneBinding();
  if (modelChanged) await dependencies.welcome.invalidateModelSelection();

  // Reconcile every explicit request, even when it matches persisted settings. This repairs an OS
  // state whose compensation failed after an earlier settings write failure.
  if (requestedLaunchAtLogin !== undefined) {
    dependencies.launchAtLogin.set(requestedLaunchAtLogin);
  }
  try {
    if (patch.app?.enabled !== undefined) await dependencies.echo.updateGeneral(patch);
    else await dependencies.state.updateSettings(patch);
  } catch (error: unknown) {
    if (requestedLaunchAtLogin !== undefined) {
      try {
        dependencies.launchAtLogin.set(before.app.launchAtLogin);
      } catch {
        // Preserve the settings failure. A later mutation/restart reconciliation retries the OS state.
      }
    }
    throw error;
  }
  return dependencies.state.getSettings();
}
