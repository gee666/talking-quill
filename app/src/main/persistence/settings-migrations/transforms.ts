import {
  VOICE_COMMANDS_MAX_UTF8_BYTES,
  VoiceCommandListSchema,
  VoiceCommandSchema,
  type VoiceCommand,
} from '../../../shared/schemas/commands';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
  GENERAL_PROFILE_ID,
  PROMPT_PROFILE_ID,
  defaultDictationProfiles,
  isReservedBindingForProfile,
} from '../../../shared/schemas/dictation-profiles';
import { ProcessingModeSchema } from '../../../shared/schemas/history';
import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  SettingsSchema,
  type Settings,
} from '../../../shared/schemas/settings';
import {
  ShortcutKeySchema,
  shortcutFromLegacyActivation,
  shortcutsConflict,
  type Shortcut,
} from '../../../shared/schemas/shortcut';
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
import type { LegacySettingsV20 } from './legacy-settings-v20';
import {
  LEGACY_DEFAULT_GENERAL_PROFILE_V21,
  LEGACY_DEFAULT_PROMPT_PROFILE_V21,
  LegacySettingsV21Schema,
  type LegacySettingsV21,
} from './legacy-settings-v21';

export function migrateRemovedLargeModel(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const migrated = structuredClone(input) as Record<string, unknown>;
  const app = migrated.app;
  if (typeof app === 'object' && app !== null && !Array.isArray(app)) {
    const legacyApp = app as Record<string, unknown>;
    legacyApp.launchAtLogin ??= DEFAULT_SETTINGS.app.launchAtLogin;
    const activationKey = ShortcutKeySchema.parse(legacyApp.activationKey);
    delete legacyApp.activationKey;
    const profiles = defaultDictationProfiles();
    const general = profiles[0];
    if (general === undefined) throw new Error('Default General profile is missing');
    profiles[0] = {
      ...general,
      shortcut: shortcutFromLegacyActivation(activationKey, false),
      processingMode: ProcessingModeSchema.parse(legacyApp.defaultProcessingMode),
    };
    migrated.dictationProfiles = profiles;
  }
  const transcription = migrated.transcription;
  if (
    typeof transcription === 'object' &&
    transcription !== null &&
    !Array.isArray(transcription)
  ) {
    const legacyTranscription = transcription as Record<string, unknown>;
    if (legacyTranscription.modelId === 'Xenova/whisper-large') {
      legacyTranscription.modelId = 'onnx-community/whisper-large-v3-turbo';
    }
    legacyTranscription.language = normalizeWhisperSourceLanguage(legacyTranscription.language);
  }
  const welcome = migrated.welcome;
  if (typeof welcome === 'object' && welcome !== null && !Array.isArray(welcome)) {
    // Earlier evidence omitted profile identity and Shift, so it is not proof
    // of any exact activation binding.
    (welcome as Record<string, unknown>).activationEvidence = null;
    (welcome as Record<string, unknown>).activationTested = false;
    migrateWelcomeStep(welcome as Record<string, unknown>);
    const modelEvidence = (welcome as Record<string, unknown>).modelEvidence;
    if (
      typeof modelEvidence === 'object' &&
      modelEvidence !== null &&
      !Array.isArray(modelEvidence) &&
      (modelEvidence as Record<string, unknown>).modelId === 'Xenova/whisper-large'
    ) {
      // The old model's revision cannot prove that the replacement model is installed.
      (welcome as Record<string, unknown>).modelEvidence = null;
    }
  }
  return migrated;
}

export function migrateFiveStepWelcome(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const migrated = structuredClone(input) as Record<string, unknown>;
  const welcome = migrated.welcome;
  if (typeof welcome === 'object' && welcome !== null && !Array.isArray(welcome)) {
    migrateWelcomeStep(welcome as Record<string, unknown>);
  }
  return migrated;
}

export function migrateShortcutChords(legacy: LegacySettingsV20): LegacySettingsV21 {
  const { activationKey: _activationKey, ...app } = structuredClone(legacy.app);
  void _activationKey;
  return LegacySettingsV21Schema.parse({
    ...structuredClone(legacy),
    schemaVersion: 21,
    app,
    dictationProfiles: legacy.dictationProfiles.map(({ activationKey, shift, ...profile }) => ({
      ...structuredClone(profile),
      shortcut: shortcutFromLegacyActivation(activationKey, shift),
    })),
  });
}

export function migrateSettingsV21(legacy: LegacySettingsV21): Settings {
  const existing = legacy.dictationProfiles.map((profile) => {
    if (legacyProfileEquals(profile, LEGACY_DEFAULT_GENERAL_PROFILE_V21)) {
      // Upgrade the dormant default prompt, but never opt an existing Raw user into Smart.
      return {
        ...structuredClone(DEFAULT_GENERAL_PROFILE),
        processingMode: profile.processingMode,
      };
    }
    if (legacyProfileEquals(profile, LEGACY_DEFAULT_PROMPT_PROFILE_V21)) {
      return structuredClone(DEFAULT_PROMPT_PROFILE);
    }
    return structuredClone(profile);
  });
  const addedBuiltIns = [
    structuredClone(DEFAULT_MARKDOWN_PROFILE),
    structuredClone(DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE),
  ];
  const addedShortcuts = addedBuiltIns.map((profile) => profile.shortcut);
  // V21 allowed custom/edited profiles to own Alt+X or an Alt+X-prefixed chord. Current validation
  // permanently reserves the canonical built-in family so reset can never create an invalid
  // collision. Keep every profile and all of its content, and move only noncanonical owners that
  // collide with the new family to the first deterministic free chord.
  const needsRepair = (profile: (typeof existing)[number]) =>
    isReservedBindingForProfile(profile.id, profile.shortcut);
  const occupied = [
    ...addedShortcuts,
    ...existing.filter((profile) => !needsRepair(profile)).map((profile) => profile.shortcut),
  ];
  const repairedExisting = existing.map((profile) => {
    if (!needsRepair(profile)) return profile;
    const shortcut = firstMigrationSafeShortcut(occupied);
    occupied.push(shortcut);
    return { ...profile, shortcut };
  });
  const existingBuiltIns = repairedExisting.filter(
    ({ id }) => id === GENERAL_PROFILE_ID || id === PROMPT_PROFILE_ID,
  );
  const customProfiles = repairedExisting.filter(
    ({ id }) => id !== GENERAL_PROFILE_ID && id !== PROMPT_PROFILE_ID,
  );
  const dictationProfiles = [...existingBuiltIns, ...addedBuiltIns, ...customProfiles];
  const general = dictationProfiles.find(({ id }) => id === GENERAL_PROFILE_ID);
  if (general === undefined) throw new Error('Migrated General profile is missing');
  return SettingsSchema.parse({
    ...structuredClone(legacy),
    // Keep the target literal so a future settings bump cannot silently skip its migration.
    schemaVersion: 22,
    // The v21 schema guarantees this compatibility mirror matches General. Preserve both so an
    // upgrade can never activate Smart/provider processing for an existing Raw user.
    app: structuredClone(legacy.app),
    transcription: {
      ...structuredClone(legacy.transcription),
      language: normalizeWhisperSourceLanguage(legacy.transcription.language),
    },
    dictationProfiles,
    welcome: {
      ...structuredClone(legacy.welcome),
      activationTested: false,
      activationEvidence: null,
    },
  });
}

export function stripDictationProfiles(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const clone = structuredClone(input) as Record<string, unknown>;
  delete clone.dictationProfiles;
  return clone;
}

export function stripTask12Fields(input: unknown): unknown {
  const stripped = stripDiagnosticLoggingField(stripDictationProfiles(input));
  if (typeof stripped !== 'object' || stripped === null || Array.isArray(stripped)) return stripped;
  const clone = structuredClone(stripped) as Record<string, unknown>;
  delete clone.welcome;
  if (typeof clone.app === 'object' && clone.app !== null && !Array.isArray(clone.app)) {
    delete (clone.app as Record<string, unknown>).launchAtLogin;
  }
  return clone;
}

function stripPiInstallationPathOnly(input: unknown): unknown {
  if (typeof input !== 'object' || input === null || Array.isArray(input)) return input;
  const clone = structuredClone(input) as Record<string, unknown>;
  if (
    typeof clone.smartProcessing === 'object' &&
    clone.smartProcessing !== null &&
    !Array.isArray(clone.smartProcessing)
  ) {
    delete (clone.smartProcessing as Record<string, unknown>).piInstallationPath;
  }
  return clone;
}

export function stripPiInstallationPath(input: unknown): unknown {
  const clone = stripPiInstallationPathOnly(input);
  if (typeof clone !== 'object' || clone === null || Array.isArray(clone)) return clone;
  const record = clone as Record<string, unknown>;
  if (
    typeof record.smartProcessing === 'object' &&
    record.smartProcessing !== null &&
    !Array.isArray(record.smartProcessing)
  ) {
    delete (record.smartProcessing as Record<string, unknown>).piExtensionsEnabled;
  }
  return record;
}

export function stripDiagnosticLoggingField(input: unknown): unknown {
  const withoutPiPath = stripPiInstallationPath(stripDictationProfiles(input));
  if (typeof withoutPiPath !== 'object' || withoutPiPath === null || Array.isArray(withoutPiPath))
    return withoutPiPath;
  const clone = structuredClone(withoutPiPath) as Record<string, unknown>;
  if (
    typeof clone.privacy === 'object' &&
    clone.privacy !== null &&
    !Array.isArray(clone.privacy)
  ) {
    delete (clone.privacy as Record<string, unknown>).diagnosticLoggingEnabled;
  }
  return clone;
}

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
    recording: legacy.recording ?? structuredClone(DEFAULT_SETTINGS.recording),
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

function legacyProfileEquals(
  profile: LegacySettingsV21['dictationProfiles'][number],
  expected: typeof LEGACY_DEFAULT_GENERAL_PROFILE_V21 | typeof LEGACY_DEFAULT_PROMPT_PROFILE_V21,
): boolean {
  return (
    profile.id === expected.id &&
    profile.name === expected.name &&
    profile.processingMode === expected.processingMode &&
    profile.smartPrompt === expected.smartPrompt &&
    JSON.stringify(profile.shortcut) === JSON.stringify(expected.shortcut)
  );
}

function firstMigrationSafeShortcut(occupied: readonly Shortcut[]): Shortcut {
  for (const shift of [false, true]) {
    for (const key of ShortcutKeySchema.options) {
      const candidate = shortcutFromLegacyActivation(key, shift);
      if (
        !isReservedBindingForProfile('migration', candidate) &&
        !occupied.some((shortcut) => shortcutsConflict(shortcut, candidate))
      ) {
        return candidate;
      }
    }
  }
  throw new Error('No migration-safe profile shortcut is available');
}

function migrateWelcomeStep(welcome: Record<string, unknown>): void {
  if (welcome.completedAt !== null && welcome.completedAt !== undefined) {
    welcome.lastStep = 5;
  } else if (welcome.lastStep === 5) {
    welcome.lastStep = 4;
  } else if (welcome.lastStep === 6) {
    welcome.lastStep = 5;
  }
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
