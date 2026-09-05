import { normalizeDefaultMicrophone } from './legacy-transforms';
export { migrateLegacy, migrateUnverifiedWelcome } from './legacy-transforms';
export {
  stripRecordingOptions,
  stripDictationProfiles,
  stripTask12Fields,
  stripPiInstallationPath,
  stripDiagnosticLoggingField,
} from './settings-stripping';
import { ProcessingModeSchema } from '../../../shared/schemas/history';
import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  SettingsSchema,
  type Settings,
} from '../../../shared/schemas/settings';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
  GENERAL_PROFILE_ID,
  PROMPT_PROFILE_ID,
  ShortcutKeySchema,
  defaultDictationProfiles,
  isReservedBindingForProfile,
  shortcutFromLegacyActivation,
  shortcutsConflict,
  shortcutsEqual,
  type Shortcut,
} from './legacy-shortcut-contract';
import { normalizeWhisperSourceLanguage } from '../../../shared/schemas/whisper-languages';
import { LegacySettingsV22Schema, type LegacySettingsV22 } from './legacy-settings-v22';
import { LegacySettingsV23Schema, type LegacySettingsV23 } from './legacy-settings-v23';
import { LegacySettingsV24Schema, type LegacySettingsV24 } from './legacy-settings-v24';
import {
  LegacySettingsV25Schema,
  LegacyV25LocalPiExtensionSourcesSchema,
  type LegacySettingsV25,
} from './legacy-settings-v25';
import { LegacySettingsV26Schema, type LegacySettingsV26 } from './legacy-settings-v26';
import { LegacySettingsV27Schema, type LegacySettingsV27 } from './legacy-settings-v27';
import { LegacySettingsV27RelaxedSchema } from './legacy-settings-v27-relaxed';
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
  const recording = migrated.recording;
  if (typeof recording === 'object' && recording !== null && !Array.isArray(recording)) {
    migrated.recording = {
      ...structuredClone(DEFAULT_SETTINGS.recording),
      ...(recording as Record<string, unknown>),
    };
    normalizeDefaultMicrophone(migrated.recording as Record<string, unknown>);
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

export function migrateSettingsV21(legacy: LegacySettingsV21): LegacySettingsV22 {
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
  return LegacySettingsV22Schema.parse({
    ...structuredClone(legacy),
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

export function migrateSettingsV22(legacy: LegacySettingsV22): LegacySettingsV23 {
  const profiles = legacy.dictationProfiles.map((profile) => structuredClone(profile));
  const promptIndex = profiles.findIndex(({ id }) => id === PROMPT_PROFILE_ID);
  profiles.splice(promptIndex + 1, 0, structuredClone(DEFAULT_PROMPT_TO_ENGLISH_PROFILE));
  return LegacySettingsV23Schema.parse({
    ...structuredClone(legacy),
    schemaVersion: 23,
    dictationProfiles: profiles,
  });
}

export function migrateSettingsV23(legacy: LegacySettingsV23): LegacySettingsV24 {
  const legacyPromptToEnglish: Shortcut = {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys: ['X', 'P', 'E'],
  };
  const legacyTranslateToEnglish: Shortcut = {
    modifiers: { ctrl: false, alt: true, shift: false, meta: false },
    keys: ['X', 'E'],
  };
  const dictationProfiles = legacy.dictationProfiles.map((profile) => {
    if (
      profile.id === DEFAULT_PROMPT_TO_ENGLISH_PROFILE.id &&
      shortcutsEqual(profile.shortcut, legacyPromptToEnglish)
    ) {
      return { ...structuredClone(profile), shortcut: DEFAULT_PROMPT_TO_ENGLISH_PROFILE.shortcut };
    }
    if (
      profile.id === DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.id &&
      shortcutsEqual(profile.shortcut, legacyTranslateToEnglish)
    ) {
      return {
        ...structuredClone(profile),
        shortcut: DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut,
      };
    }
    return structuredClone(profile);
  });
  return LegacySettingsV24Schema.parse({
    ...structuredClone(legacy),
    schemaVersion: 24,
    dictationProfiles,
  });
}

export function migrateSettingsV24(legacy: LegacySettingsV24): LegacySettingsV25 {
  return LegacySettingsV25Schema.parse({
    ...structuredClone(legacy),
    schemaVersion: 25,
    recording: {
      ...structuredClone(DEFAULT_SETTINGS.recording),
      ...structuredClone(legacy.recording),
    },
  });
}

export function migrateSettingsV25(legacy: LegacySettingsV25): LegacySettingsV26 {
  const migrated = structuredClone(legacy);
  const piDraft = migrated.smartProcessing.providers.pi;
  if (
    piDraft?.piExtensionSources !== undefined &&
    !LegacyV25LocalPiExtensionSourcesSchema.safeParse(piDraft.piExtensionSources).success
  ) {
    piDraft.piExtensionSources = [];
  }
  return LegacySettingsV26Schema.parse({
    ...migrated,
    schemaVersion: 26,
  });
}

export function migrateSettingsV26(legacy: LegacySettingsV26): LegacySettingsV27 {
  const migrated = structuredClone(legacy);
  normalizeDefaultMicrophone(migrated.recording);
  return LegacySettingsV27Schema.parse({
    ...migrated,
    schemaVersion: 27,
  });
}

export function migrateSettingsV27(input: Readonly<Record<string, unknown>>): Settings {
  const released = LegacySettingsV27Schema.safeParse(input);
  if (released.success) {
    return SettingsSchema.parse({
      ...structuredClone(released.data),
      schemaVersion: SETTINGS_SCHEMA_VERSION,
    });
  }

  // A development build briefly wrote the relaxed shared-prefix shape with the released v27
  // discriminator. Rescue only data that is otherwise an exact current settings object; malformed
  // v27 files must still take the normal corruption-recovery path.
  const relaxed = LegacySettingsV27RelaxedSchema.parse(input);
  return SettingsSchema.parse({
    ...structuredClone(relaxed),
    schemaVersion: SETTINGS_SCHEMA_VERSION,
  });
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
