import { SETTINGS_SCHEMA_VERSION, SettingsSchema } from '../../shared/schemas/settings';
import type { SettingsMigrations } from './settings-store';
import { LEGACY_SETTINGS_V1_TO_V12_SCHEMAS } from './settings-migrations/legacy-settings-v1-v12';
import { LEGACY_SETTINGS_V13_TO_V18_SCHEMAS } from './settings-migrations/legacy-settings-v13-v18';
import { LegacySettingsV19Schema } from './settings-migrations/legacy-settings-v19';
import { LegacySettingsV20Schema } from './settings-migrations/legacy-settings-v20';
import {
  migrateFiveStepWelcome,
  migrateLegacy,
  migrateShortcutChords,
  migrateRemovedLargeModel,
  migrateUnverifiedWelcome,
  stripDiagnosticLoggingField,
  stripDictationProfiles,
  stripPiInstallationPath,
  stripTask12Fields,
} from './settings-migrations/transforms';

export const SETTINGS_MIGRATIONS: SettingsMigrations = Object.freeze({
  1: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[1].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  2: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[2].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  3: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[3].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  4: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[4].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  5: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[5].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  6: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[6].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  7: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[7].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  8: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[8].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  9: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[9].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  10: (input) =>
    migrateLegacy(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[10].parse(stripTask12Fields(stripDictationProfiles(input))),
    ),
  11: (input) =>
    migrateUnverifiedWelcome(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[11].parse(
        stripDiagnosticLoggingField(stripDictationProfiles(input)),
      ),
    ),
  12: (input) =>
    migrateUnverifiedWelcome(
      LEGACY_SETTINGS_V1_TO_V12_SCHEMAS[12].parse(
        stripDiagnosticLoggingField(stripDictationProfiles(input)),
      ),
    ),
  13: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[13].parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        privacy: { ...legacy.privacy, diagnosticLoggingEnabled: false },
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  14: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[14].parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  15: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[15].parse(
      stripPiInstallationPath(stripDictationProfiles(input)),
    );
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing: { ...legacy.smartProcessing, piInstallationPath: null },
      }),
    );
  },
  16: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[16].parse(stripDictationProfiles(input));
    return SettingsSchema.parse(
      migrateRemovedLargeModel({ ...legacy, schemaVersion: SETTINGS_SCHEMA_VERSION }),
    );
  },
  17: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[17].parse(stripDictationProfiles(input));
    const { piExtensionsEnabled, ...smartProcessing } = legacy.smartProcessing;
    void piExtensionsEnabled;
    return SettingsSchema.parse(
      migrateRemovedLargeModel({
        ...legacy,
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        smartProcessing,
      }),
    );
  },
  18: (input) => {
    const legacy = LEGACY_SETTINGS_V13_TO_V18_SCHEMAS[18].parse(stripDictationProfiles(input));
    return SettingsSchema.parse(
      migrateRemovedLargeModel({ ...legacy, schemaVersion: SETTINGS_SCHEMA_VERSION }),
    );
  },
  19: (input) =>
    LegacySettingsV20Schema.parse(
      migrateFiveStepWelcome({
        ...LegacySettingsV19Schema.parse(input),
        schemaVersion: 20,
      }),
    ),
  20: (input) => migrateShortcutChords(LegacySettingsV20Schema.parse(input)),
});
