import {
  SETTINGS_SCHEMA_VERSION,
  SettingsSchema,
  type Settings,
  type SettingsPatch,
} from '../../shared/schemas/settings';
import { GENERAL_PROFILE_ID } from '../../shared/schemas/dictation-profiles';

export function mergeSettings(current: Settings, patch: SettingsPatch): Settings {
  const providerPatches = patch.smartProcessing?.providers ?? {};
  const providers = { ...current.smartProcessing.providers };
  for (const [providerId, providerPatch] of Object.entries(providerPatches)) {
    providers[providerId as keyof typeof providers] = {
      ...providers[providerId as keyof typeof providers],
      ...providerPatch,
    };
  }
  for (const [providerId, replacement] of Object.entries(
    patch.smartProcessing?.providerReplacements ?? {},
  )) {
    providers[providerId as keyof typeof providers] = replacement;
  }
  const dictationProfiles = patch.dictationProfiles ?? current.dictationProfiles;
  const general = dictationProfiles.find((profile) => profile.id === GENERAL_PROFILE_ID);
  if (general === undefined) throw new Error('The General dictation profile is required');
  return SettingsSchema.parse({
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    app: {
      ...current.app,
      ...patch.app,
      defaultProcessingMode: general.processingMode,
    },
    recording: { ...current.recording, ...patch.recording },
    transcription: { ...current.transcription, ...patch.transcription },
    dictationProfiles,
    privacy: { ...current.privacy, ...patch.privacy },
    smartProcessing: {
      selectedProviderId:
        patch.smartProcessing?.selectedProviderId ?? current.smartProcessing.selectedProviderId,
      providers,
      credentialEpochs: {
        ...current.smartProcessing.credentialEpochs,
        ...patch.smartProcessing?.credentialEpochs,
      },
      piInstallationPath:
        patch.smartProcessing?.piInstallationPath !== undefined
          ? patch.smartProcessing.piInstallationPath
          : current.smartProcessing.piInstallationPath,
      onScreenAwarenessEnabled:
        patch.smartProcessing?.onScreenAwarenessEnabled ??
        current.smartProcessing.onScreenAwarenessEnabled,
      visionOverrides:
        patch.smartProcessing?.visionOverrides ?? current.smartProcessing.visionOverrides,
    },
    voiceCommands: patch.voiceCommands ?? current.voiceCommands,
    customVocabulary: patch.customVocabulary ?? current.customVocabulary,
    welcome: { ...current.welcome, ...patch.welcome },
  });
}
