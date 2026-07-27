import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import { SETTINGS_SCHEMA_VERSION, SettingsSchema } from '../../app/src/shared/schemas/settings';

interface RawFixture {
  readonly name: string;
  readonly source: Readonly<Record<string, unknown>>;
  readonly version: number;
}

const fixtureDirectory = join(import.meta.dirname, '../fixtures/settings-migrations');
const PRIMARY_RAW_FIXTURES = [
  ...readRawFixtures('legacy-v1-v12.json'),
  ...readRawFixtures('legacy-v13-v18.json'),
  ...readRawFixtures('legacy-v19-v20.json'),
];
const RAW_FIXTURES = [...PRIMARY_RAW_FIXTURES, ...readRawFixtures('legacy-forked-versions.json')];

describe('frozen settings migrations', () => {
  it('keeps the public migration table complete and frozen', () => {
    expect(Object.keys(SETTINGS_MIGRATIONS).map(Number)).toEqual(
      Array.from({ length: 20 }, (_, index) => index + 1),
    );
    expect(Object.isFrozen(SETTINGS_MIGRATIONS)).toBe(true);
    expect(PRIMARY_RAW_FIXTURES.map((fixture) => fixture.version)).toEqual(
      Array.from({ length: 20 }, (_, index) => index + 1),
    );
  });

  it('makes v19 emit literal v20 before the v20 to v21 shortcut migration', () => {
    const fixture = readRawFixtures('legacy-v19-v20.json').find(({ version }) => version === 19);
    if (fixture === undefined) throw new Error('Missing v19 fixture');

    const v20 = SETTINGS_MIGRATIONS[19]?.(fixture.source);
    expect(v20).toMatchObject({ schemaVersion: 20, app: { activationKey: 'Q' } });
    expect(v20).toHaveProperty('dictationProfiles.0.activationKey', 'Q');
    expect(v20).toHaveProperty('dictationProfiles.0.shift', true);

    const v21 = SettingsSchema.parse(
      SETTINGS_MIGRATIONS[20]?.(v20 as Readonly<Record<string, unknown>>),
    );
    expect(v21.schemaVersion).toBe(21);
    expect(v21.app).not.toHaveProperty('activationKey');
    expect(v21.dictationProfiles[0]?.shortcut).toEqual({
      modifiers: { ctrl: false, alt: true, shift: true, meta: false },
      keys: ['Q'],
    });
  });

  it.each(RAW_FIXTURES)('migrates raw $name settings without recovery', async (fixture) => {
    const sourceSnapshot = structuredClone(fixture.source);
    let persisted: unknown;
    const preserveInvalid = vi.fn(() => Promise.resolve(null));
    const write = vi.fn((_path: string, value: unknown) => {
      persisted = structuredClone(value);
      return Promise.resolve();
    });
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: {
        read: () => Promise.resolve(JSON.stringify(fixture.source)),
        write,
        preserveInvalid,
      },
    });

    await store.initialize();

    expect(fixture.source).toEqual(sourceSnapshot);
    expect(store.getDiagnostic()).toBeNull();
    expect(preserveInvalid).not.toHaveBeenCalled();
    expect(write).toHaveBeenCalledOnce();
    const migrated = SettingsSchema.parse(store.get());
    expect(migrated.schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(migrated.app).toMatchObject({ enabled: false, closeToTray: false });
    expectFrozenCanariesToSurvive(fixture, migrated);

    const legacyEndpoint = readLegacyGenericEndpoint(fixture.source);
    if (legacyEndpoint !== null && /^(?:file|ftp):/.test(legacyEndpoint)) {
      expect(migrated.smartProcessing.providers['generic-openai']?.baseUrl).toBe(legacyEndpoint);
      expect(() => new ProviderConfigService(store).get('generic-openai')).toThrow();
    }

    const restarted = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve(JSON.stringify(persisted)),
        write: () => Promise.resolve(),
        preserveInvalid,
      },
    });
    await restarted.initialize();
    expect(restarted.getDiagnostic()).toBeNull();
    expect(restarted.get()).toEqual(migrated);
  });

  it('keeps historical source contracts independent from current shared schemas', () => {
    for (const filename of [
      'legacy-settings-contracts.ts',
      'legacy-settings-v1-v12.ts',
      'legacy-settings-v13-v18.ts',
      'legacy-settings-v19.ts',
      'legacy-settings-v20.ts',
    ]) {
      const source = readFileSync(
        join(import.meta.dirname, '../../app/src/main/persistence/settings-migrations', filename),
        'utf8',
      );
      expect(source).not.toMatch(/shared[\\/]schemas|shared[\\/]helper/);
    }
  });
});

function readRawFixtures(filename: string): RawFixture[] {
  const parsed: unknown = JSON.parse(readFileSync(join(fixtureDirectory, filename), 'utf8'));
  if (!isRecord(parsed)) throw new Error(`Invalid settings fixture collection: ${filename}`);
  return Object.entries(parsed).map(([name, source]) => {
    if (!isRecord(source) || typeof source.schemaVersion !== 'number') {
      throw new Error(`Invalid raw settings fixture: ${name}`);
    }
    return { name, source, version: source.schemaVersion };
  });
}

function expectFrozenCanariesToSurvive(
  fixture: RawFixture,
  migrated: ReturnType<typeof SettingsSchema.parse>,
): void {
  const { source, version } = fixture;
  const app = readRecord(source.app);
  if (typeof app?.activationKey === 'string') {
    expect(migrated.app).not.toHaveProperty('activationKey');
    const sourceProfiles = Array.isArray(source.dictationProfiles)
      ? source.dictationProfiles.filter(isRecord)
      : [];
    const sourceGeneral = sourceProfiles.find((profile) => profile.id === 'general');
    expect(migrated.dictationProfiles.find((profile) => profile.id === 'general')).toMatchObject({
      shortcut: {
        modifiers: {
          ctrl: false,
          alt: true,
          shift: sourceGeneral?.shift === true,
          meta: false,
        },
        keys: [app.activationKey],
      },
      processingMode: app.defaultProcessingMode,
    });
    for (const sourceProfile of sourceProfiles) {
      const migratedProfile = migrated.dictationProfiles.find(
        (profile) => profile.id === sourceProfile.id,
      );
      expect(migratedProfile?.shortcut).toEqual({
        modifiers: {
          ctrl: false,
          alt: true,
          shift: sourceProfile.shift === true,
          meta: false,
        },
        keys: [sourceProfile.activationKey],
      });
    }
  }
  if (typeof app?.widgetSize === 'string') expect(migrated.app.widgetSize).toBe(app.widgetSize);
  if (typeof app?.soundsEnabled === 'boolean') {
    expect(migrated.app.soundsEnabled).toBe(app.soundsEnabled);
  }
  if (typeof app?.launchAtLogin === 'boolean') {
    expect(migrated.app.launchAtLogin).toBe(app.launchAtLogin);
  } else if (version >= 14) {
    expect(migrated.app.launchAtLogin).toBe(false);
  }

  const recording = readRecord(source.recording);
  if (typeof recording?.preferredMicrophoneId === 'string') {
    expect(migrated.recording.preferredMicrophoneId).toBe(recording.preferredMicrophoneId);
  }
  const transcription = readRecord(source.transcription);
  if (typeof transcription?.language === 'string') {
    expect(migrated.transcription.language).toBe(transcription.language);
  }
  if (transcription?.modelId === 'Xenova/whisper-large') {
    expect(migrated.transcription.modelId).toBe('onnx-community/whisper-large-v3-turbo');
  }

  const privacy = readRecord(source.privacy);
  for (const key of [
    'historyEnabled',
    'historyRetentionDays',
    'retainSmartScreenshots',
    'diagnosticLoggingEnabled',
  ] as const) {
    if (privacy?.[key] !== undefined) expect(migrated.privacy[key]).toBe(privacy[key]);
  }

  const smartProcessing = readRecord(source.smartProcessing);
  const credentialEpochs = readRecord(smartProcessing?.credentialEpochs);
  if (credentialEpochs !== null) {
    expect(migrated.smartProcessing.credentialEpochs).toEqual(credentialEpochs);
  }
  if (typeof smartProcessing?.onScreenAwarenessEnabled === 'boolean') {
    expect(migrated.smartProcessing.onScreenAwarenessEnabled).toBe(
      smartProcessing.onScreenAwarenessEnabled,
    );
  }
  if (Array.isArray(smartProcessing?.visionOverrides)) {
    expect(migrated.smartProcessing.visionOverrides).toEqual(smartProcessing.visionOverrides);
  }
  if (smartProcessing?.piInstallationPath !== undefined) {
    expect(migrated.smartProcessing.piInstallationPath).toBe(smartProcessing.piInstallationPath);
  }
  const sourceProviders = readRecord(smartProcessing?.providers);
  const sourceGenericOpenAi = readRecord(sourceProviders?.['generic-openai']);
  if (sourceGenericOpenAi !== null) {
    expect(migrated.smartProcessing.providers['generic-openai']).toEqual(sourceGenericOpenAi);
  }

  if (Array.isArray(source.voiceCommands)) {
    expect(migrated.voiceCommands).toEqual(source.voiceCommands);
  }
  if (Array.isArray(source.customVocabulary)) {
    expect(migrated.customVocabulary).toEqual(source.customVocabulary);
  }

  const welcome = readRecord(source.welcome);
  if (welcome?.activationEvidence !== undefined) {
    expect(migrated.welcome).toMatchObject({ activationTested: false, activationEvidence: null });
  }
  if (readRecord(welcome?.modelEvidence)?.modelId === 'Xenova/whisper-large') {
    expect(migrated.welcome.modelEvidence).toBeNull();
  }
}

function readLegacyGenericEndpoint(source: Readonly<Record<string, unknown>>): string | null {
  const smartProcessing = readRecord(source.smartProcessing);
  const providers = readRecord(smartProcessing?.providers);
  const genericOpenAi = readRecord(providers?.['generic-openai']);
  return typeof genericOpenAi?.baseUrl === 'string' ? genericOpenAi.baseUrl : null;
}

function readRecord(value: unknown): Readonly<Record<string, unknown>> | null {
  return isRecord(value) ? value : null;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}
