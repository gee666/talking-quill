import {
  VOICE_COMMANDS_MAX_UTF8_BYTES,
  VoiceCommandListSchema,
  VoiceCommandSchema,
  type VoiceCommand,
} from '../../../shared/schemas/commands';
import { ProcessingModeSchema } from '../../../shared/schemas/history';
import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  type Settings,
} from '../../../shared/schemas/settings';
import {
  ShortcutKeySchema,
  defaultDictationProfiles,
  shortcutFromLegacyActivation,
} from './legacy-shortcut-contract';
import { normalizeWhisperSourceLanguage } from '../../../shared/schemas/whisper-languages';
import { utf8ByteLength } from '../../../shared/schemas/text-bounds';
import {
  VOCABULARY_TOTAL_MAX_UTF8_BYTES,
  VocabularyEntrySchema,
  VocabularyListSchema,
  type VocabularyEntry,
} from '../../../shared/schemas/vocabulary';
import type {
  LegacySettingsBase,
  LegacySettingsWithWelcomeProgress,
} from './legacy-settings-contracts';

export function migrateUnverifiedWelcome(legacy: LegacySettingsWithWelcomeProgress): Settings {
  const migrated = migrateLegacy(legacy);
  // Completion is durable historical UI state, while partial progress still resumes at the
  // earliest evidence-bearing step without manufacturing microphone/model/helper proof.
  const completedAt = legacy.welcome.completedAt;
  migrated.welcome = {
    ...structuredClone(DEFAULT_SETTINGS.welcome),
    lastStep: completedAt === null ? (legacy.welcome.lastStep === 1 ? 1 : 2) : 5,
    completedAt,
  };
  return migrated;
}

export function migrateLegacy(legacy: LegacySettingsBase): Settings {
  const legacyApp = legacy.app as Record<string, unknown>;
  const profiles = defaultDictationProfiles();
  if (legacyApp.activationKey !== undefined) {
    const general = profiles[0];
    if (general === undefined) throw new Error('Default General profile is missing');
    profiles[0] = {
      ...general,
      shortcut: shortcutFromLegacyActivation(
        ShortcutKeySchema.parse(legacyApp.activationKey),
        false,
      ),
      processingMode: ProcessingModeSchema.parse(legacyApp.defaultProcessingMode),
    };
  }
  return {
    schemaVersion: SETTINGS_SCHEMA_VERSION,
    app: {
      ...structuredClone(DEFAULT_SETTINGS.app),
      enabled: legacy.app.enabled,
      closeToTray: legacy.app.closeToTray,
      defaultProcessingMode:
        'defaultProcessingMode' in legacy.app
          ? legacy.app.defaultProcessingMode
          : DEFAULT_SETTINGS.app.defaultProcessingMode,
      ...('widgetSize' in legacy.app ? { widgetSize: legacy.app.widgetSize } : {}),
      ...('soundsEnabled' in legacy.app ? { soundsEnabled: legacy.app.soundsEnabled } : {}),
      launchAtLogin:
        'launchAtLogin' in legacy.app && legacy.app.launchAtLogin !== undefined
          ? legacy.app.launchAtLogin
          : DEFAULT_SETTINGS.app.launchAtLogin,
    },
    recording: normalizeLegacyRecording(legacy.recording),
    transcription: migrateLegacyTranscription(legacy.transcription),
    dictationProfiles: profiles,
    privacy: {
      ...structuredClone(DEFAULT_SETTINGS.privacy),
      ...(legacy.privacy ?? {}),
    },
    smartProcessing:
      legacy.smartProcessing === undefined
        ? structuredClone(DEFAULT_SETTINGS.smartProcessing)
        : migrateLegacySmartProcessing(legacy.smartProcessing),
    voiceCommands: retainBoundedCommands(
      structuredClone(legacy.voiceCommands ?? DEFAULT_SETTINGS.voiceCommands),
    ),
    customVocabulary: retainBoundedVocabulary(
      structuredClone(legacy.customVocabulary ?? DEFAULT_SETTINGS.customVocabulary),
    ),
    welcome: structuredClone(DEFAULT_SETTINGS.welcome),
  };
}

function normalizeLegacyRecording(
  recording: LegacySettingsBase['recording'],
): Settings['recording'] {
  const migrated = {
    ...structuredClone(DEFAULT_SETTINGS.recording),
    ...(recording ?? {}),
  };
  normalizeDefaultMicrophone(migrated);
  return migrated;
}

export function normalizeDefaultMicrophone(recording: Record<string, unknown>): void {
  if (recording.preferredMicrophoneId === 'default') recording.preferredMicrophoneId = null;
}

function migrateLegacyTranscription(
  legacy: LegacySettingsBase['transcription'],
): Settings['transcription'] {
  if (legacy === undefined) return structuredClone(DEFAULT_SETTINGS.transcription);
  return {
    modelId:
      legacy.modelId === 'Xenova/whisper-large'
        ? 'onnx-community/whisper-large-v3-turbo'
        : legacy.modelId,
    language: normalizeWhisperSourceLanguage(legacy.language),
  };
}

function migrateLegacySmartProcessing(
  legacy: NonNullable<LegacySettingsBase['smartProcessing']>,
): Settings['smartProcessing'] {
  const providers = structuredClone(legacy.providers);
  const bedrock = providers.bedrock;
  if (bedrock !== undefined && bedrock.region === undefined) {
    providers.bedrock = { ...bedrock, region: 'us-west-2' };
  }
  return {
    selectedProviderId: legacy.selectedProviderId,
    providers,
    credentialEpochs: structuredClone(legacy.credentialEpochs ?? {}),
    piInstallationPath: null,
    onScreenAwarenessEnabled: legacy.onScreenAwarenessEnabled ?? false,
    visionOverrides: (legacy.visionOverrides ?? []).map((override) => ({ ...override })),
  };
}

function retainBoundedCommands(commands: readonly LegacySettingsBaseCommand[]): VoiceCommand[] {
  const retained: VoiceCommand[] = [];
  let bytes = 0;
  for (const command of commands) {
    const parsed = VoiceCommandSchema.safeParse(command);
    if (!parsed.success) continue;
    const nextBytes =
      bytes + utf8ByteLength(parsed.data.trigger) + utf8ByteLength(parsed.data.snippet);
    if (nextBytes > VOICE_COMMANDS_MAX_UTF8_BYTES) continue;
    retained.push(parsed.data);
    bytes = nextBytes;
  }
  return VoiceCommandListSchema.parse(retained);
}

type LegacySettingsBaseCommand = NonNullable<LegacySettingsBase['voiceCommands']>[number];

function retainBoundedVocabulary(
  entries: readonly LegacySettingsBaseVocabularyEntry[],
): VocabularyEntry[] {
  const retained: VocabularyEntry[] = [];
  let bytes = 0;
  for (const entry of entries) {
    const parsed = VocabularyEntrySchema.safeParse(entry);
    if (!parsed.success) continue;
    const nextBytes = bytes + utf8ByteLength(parsed.data.value);
    if (nextBytes > VOCABULARY_TOTAL_MAX_UTF8_BYTES) continue;
    retained.push(parsed.data);
    bytes = nextBytes;
  }
  return VocabularyListSchema.parse(retained);
}

type LegacySettingsBaseVocabularyEntry = NonNullable<
  LegacySettingsBase['customVocabulary']
>[number];
