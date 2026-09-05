import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import {
  LEGACY_DEFAULT_GENERAL_PROFILE_V21,
  LEGACY_DEFAULT_PROMPT_PROFILE_V21,
  LegacySettingsV21Schema,
} from '../../app/src/main/persistence/settings-migrations/legacy-settings-v21';
import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  SettingsSchema,
} from '../../app/src/shared/schemas/settings';
import { normalizeWhisperSourceLanguage } from '../../app/src/shared/schemas/whisper-languages';

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
  ...readRawFixtures('legacy-v21.json'),
];
const RAW_FIXTURES = [...PRIMARY_RAW_FIXTURES, ...readRawFixtures('legacy-forked-versions.json')];

function migrateV21ToCurrent(input: Readonly<Record<string, unknown>>) {
  const v22 = SETTINGS_MIGRATIONS[21]?.(input);
  if (typeof v22 !== 'object' || v22 === null) throw new Error('V21 migration did not emit v22');
  const v23 = SETTINGS_MIGRATIONS[22]?.(v22 as Readonly<Record<string, unknown>>);
  if (typeof v23 !== 'object' || v23 === null) throw new Error('V22 migration did not emit v23');
  const v24 = SETTINGS_MIGRATIONS[23]?.(v23 as Readonly<Record<string, unknown>>);
  if (typeof v24 !== 'object' || v24 === null) throw new Error('V23 migration did not emit v24');
  const v25 = SETTINGS_MIGRATIONS[24]?.(v24 as Readonly<Record<string, unknown>>);
  if (typeof v25 !== 'object' || v25 === null) throw new Error('V24 migration did not emit v25');
  return migrateV25ToCurrent(v25 as Readonly<Record<string, unknown>>);
}

function migrateV25ToCurrent(input: Readonly<Record<string, unknown>>) {
  const v26 = SETTINGS_MIGRATIONS[25]?.(input);
  if (typeof v26 !== 'object' || v26 === null) throw new Error('V25 migration did not emit v26');
  return migrateV26ToCurrent(v26 as Readonly<Record<string, unknown>>);
}

function migrateV26ToCurrent(input: Readonly<Record<string, unknown>>) {
  const v27 = SETTINGS_MIGRATIONS[26]?.(input);
  if (typeof v27 !== 'object' || v27 === null) throw new Error('V26 migration did not emit v27');
  const v28 = SETTINGS_MIGRATIONS[27]?.(v27 as Readonly<Record<string, unknown>>);
  if (typeof v28 !== 'object' || v28 === null) throw new Error('V27 migration did not emit v28');
  return SettingsSchema.parse(v28);
}

describe('frozen settings migrations', () => {
  it('keeps the public migration table complete and frozen', () => {
    expect(Object.keys(SETTINGS_MIGRATIONS).map(Number)).toEqual(
      Array.from({ length: 27 }, (_, index) => index + 1),
    );
    expect(Object.isFrozen(SETTINGS_MIGRATIONS)).toBe(true);
    expect(PRIMARY_RAW_FIXTURES.map((fixture) => fixture.version)).toEqual(
      Array.from({ length: 21 }, (_, index) => index + 1),
    );
  });

  it('adds privacy-safe recording defaults to the released v24 contract', () => {
    const source = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    source.schemaVersion = 24;
    const recording = source.recording;
    if (typeof recording !== 'object' || recording === null || Array.isArray(recording)) {
      throw new Error('Missing recording settings');
    }
    delete (recording as Record<string, unknown>).autoSubmitOnSilence;
    delete (recording as Record<string, unknown>).includeSystemAudio;

    const v25 = SETTINGS_MIGRATIONS[24]?.(source);
    expect(v25).toMatchObject({ schemaVersion: 25 });
    const migrated = migrateV25ToCurrent(v25 as Readonly<Record<string, unknown>>);
    expect(migrated.recording).toMatchObject({
      autoSubmitOnSilence: true,
      includeSystemAudio: false,
    });
  });

  it('preserves released v25 drafts without activating dormant prerelease npm extensions', () => {
    const released = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    released.schemaVersion = 25;
    const releasedMigrated = migrateV25ToCurrent(released);
    expect(releasedMigrated.schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(releasedMigrated.smartProcessing.providers.pi?.piExtensionSources).toBeUndefined();

    const prerelease = structuredClone(released);
    const smartProcessing = readRecord(prerelease.smartProcessing);
    if (smartProcessing === null) throw new Error('Missing Smart processing settings');
    (smartProcessing as Record<string, unknown>).selectedProviderId = 'pi';
    const providers = readRecord(smartProcessing.providers);
    if (providers === null) throw new Error('Missing provider drafts');
    (providers as Record<string, unknown>).pi = {
      modelId: 'p/model',
      thinking: 'off',
      piExtensionSources: ['./extensions/first.ts', 'npm:@prerelease/pi-extension'],
    };

    const prereleaseMigrated = migrateV25ToCurrent(prerelease);
    expect(prereleaseMigrated.smartProcessing.providers.pi).toEqual({
      modelId: 'p/model',
      thinking: 'off',
      piExtensionSources: [],
    });
  });

  it('sanitizes an inactive prerelease Pi draft while preserving the selected provider', () => {
    const prerelease = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    prerelease.schemaVersion = 25;
    const smartProcessing = readRecord(prerelease.smartProcessing);
    if (smartProcessing === null) throw new Error('Missing Smart processing settings');
    (smartProcessing as Record<string, unknown>).selectedProviderId = 'ollama';
    const providers = readRecord(smartProcessing.providers);
    if (providers === null) throw new Error('Missing provider drafts');
    (providers as Record<string, unknown>).pi = {
      modelId: 'p/model',
      thinking: 'off',
      piExtensionSources: ['git:github.com/prerelease/pi-extension'],
    };

    const migrated = migrateV25ToCurrent(prerelease);
    expect(migrated.smartProcessing.selectedProviderId).toBe('ollama');
    expect(migrated.smartProcessing.providers.pi).toEqual({
      modelId: 'p/model',
      thinking: 'off',
      piExtensionSources: [],
    });
  });

  it('canonicalizes the literal default microphone without changing explicit selections', () => {
    const legacyDefault = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    legacyDefault.schemaVersion = 26;
    const defaultRecording = readRecord(legacyDefault.recording);
    if (defaultRecording === null) throw new Error('Missing recording settings');
    (defaultRecording as Record<string, unknown>).preferredMicrophoneId = 'default';

    const migratedDefault = migrateV26ToCurrent(legacyDefault);
    expect(migratedDefault.recording.preferredMicrophoneId).toBeNull();

    const explicit = structuredClone(legacyDefault);
    const explicitRecording = readRecord(explicit.recording);
    if (explicitRecording === null) throw new Error('Missing recording settings');
    (explicitRecording as Record<string, unknown>).preferredMicrophoneId = 'studio-microphone';
    const migratedExplicit = migrateV26ToCurrent(explicit);
    expect(migratedExplicit.recording.preferredMicrophoneId).toBe('studio-microphone');
  });

  it('makes v19 emit literal v20 and v20 emit frozen v21 before current migration', () => {
    const fixture = readRawFixtures('legacy-v19-v20.json').find(({ version }) => version === 19);
    if (fixture === undefined) throw new Error('Missing v19 fixture');

    const v20 = SETTINGS_MIGRATIONS[19]?.(fixture.source);
    expect(v20).toMatchObject({ schemaVersion: 20, app: { activationKey: 'Q' } });
    expect(v20).toHaveProperty('dictationProfiles.0.activationKey', 'Q');
    expect(v20).toHaveProperty('dictationProfiles.0.shift', true);

    const v21 = LegacySettingsV21Schema.parse(
      SETTINGS_MIGRATIONS[20]?.(v20 as Readonly<Record<string, unknown>>),
    );
    expect(v21.schemaVersion).toBe(21);
    expect(v21.app).not.toHaveProperty('activationKey');
    expect(v21.dictationProfiles[0]?.shortcut).toEqual({
      modifiers: { ctrl: false, alt: true, shift: true, meta: false },
      keys: ['Q'],
    });

    const current = migrateV21ToCurrent(v21);
    expect(current.schemaVersion).toBe(28);
    expect(current.transcription.language).toBe('fr');
    expect(current.dictationProfiles.map(({ id }) => id)).toEqual([
      'general',
      'prompt',
      'prompt-to-english',
      'markdown',
      'translate-to-english',
      '11111111-1111-4111-8111-111111111111',
    ]);
  });

  it('moves only the old English built-in defaults to unambiguous one-letter suffixes', () => {
    const source = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    source.schemaVersion = 23;
    const profiles = source.dictationProfiles;
    if (!Array.isArray(profiles)) throw new Error('Missing v23 profiles');
    const profileValues = profiles as unknown[];
    const promptToEnglish = profileValues.find(
      (profile) => readRecord(profile)?.id === 'prompt-to-english',
    );
    const translateToEnglish = profileValues.find(
      (profile) => readRecord(profile)?.id === 'translate-to-english',
    );
    if (!isRecord(promptToEnglish) || !isRecord(translateToEnglish)) {
      throw new Error('Missing English built-in profiles');
    }
    (promptToEnglish as Record<string, unknown>).shortcut = {
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['X', 'P', 'E'],
    };
    (translateToEnglish as Record<string, unknown>).shortcut = {
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['X', 'E'],
    };

    const v24 = SETTINGS_MIGRATIONS[23]?.(source);
    if (typeof v24 !== 'object' || v24 === null) throw new Error('V23 migration did not emit v24');
    const v25 = SETTINGS_MIGRATIONS[24]?.(v24 as Readonly<Record<string, unknown>>);
    if (typeof v25 !== 'object' || v25 === null) throw new Error('V24 migration did not emit v25');
    const migrated = migrateV25ToCurrent(v25 as Readonly<Record<string, unknown>>);
    expect(
      migrated.dictationProfiles.find(({ id }) => id === 'prompt-to-english')?.shortcut.keys,
    ).toEqual(['X', 'Q']);
    expect(
      migrated.dictationProfiles.find(({ id }) => id === 'translate-to-english')?.shortcut.keys,
    ).toEqual(['X', 'T']);
  });

  it('upgrades privacy-safe v21 defaults without enabling Smart for a completed cloud setup', () => {
    const fixture = readRawFixtures('legacy-v21.json')[0];
    if (fixture === undefined) throw new Error('Missing v21 fixture');
    const source = structuredClone(fixture.source);
    const profiles = source.dictationProfiles;
    if (!Array.isArray(profiles)) throw new Error('Missing v21 profiles');
    profiles[0] = structuredClone(LEGACY_DEFAULT_GENERAL_PROFILE_V21);
    profiles[1] = structuredClone(LEGACY_DEFAULT_PROMPT_PROFILE_V21);
    const app = readRecord(source.app);
    if (app === null) throw new Error('Missing v21 app settings');
    (app as Record<string, unknown>).enabled = true;
    const welcome = readRecord(source.welcome);
    if (welcome === null) throw new Error('Missing v21 Welcome settings');
    (welcome as Record<string, unknown>).completedAt = 1_700_000_000_000;
    (source as Record<string, unknown>).smartProcessing = {
      selectedProviderId: 'openai',
      providers: { openai: { modelId: 'gpt-4.1' } },
      credentialEpochs: { openai: 7 },
      piInstallationPath: null,
      onScreenAwarenessEnabled: false,
      visionOverrides: [],
    };
    const legacy = LegacySettingsV21Schema.parse(source);

    const migrated = migrateV21ToCurrent(legacy);
    expect(migrated.welcome.completedAt).toBe(1_700_000_000_000);
    expect(migrated.smartProcessing).toMatchObject({
      selectedProviderId: 'openai',
      providers: { openai: { modelId: 'gpt-4.1' } },
      credentialEpochs: { openai: 7 },
    });
    expect(migrated.app).toMatchObject({ enabled: true, defaultProcessingMode: 'raw' });
    expect(migrated.dictationProfiles.find(({ id }) => id === 'general')).toMatchObject({
      shortcut: {
        modifiers: { ctrl: false, alt: true, shift: false, meta: false },
        keys: ['X'],
      },
      processingMode: 'raw',
      smartPrompt: 'Clean up and format the transcript while preserving its source language.',
    });
    const prompt = migrated.dictationProfiles.find(({ id }) => id === 'prompt');
    expect(prompt).toMatchObject({
      shortcut: {
        modifiers: { ctrl: false, alt: true, shift: false, meta: false },
        keys: ['X', 'P'],
      },
    });
    expect(prompt?.smartPrompt).toContain('Preserve the source language.');
    expect(migrated.dictationProfiles.find(({ name }) => name === 'Old Alt T')?.smartPrompt).toBe(
      'Keep this custom prompt',
    );
  });

  it('migrates a full v21 profile list without loss and resolves new reserved collisions', () => {
    const fixture = readRawFixtures('legacy-v21.json')[0];
    if (fixture === undefined) throw new Error('Missing v21 fixture');
    const source = LegacySettingsV21Schema.parse(fixture.source);
    const migrated = migrateV21ToCurrent(source);

    for (const legacyProfile of source.dictationProfiles) {
      const migratedProfile = migrated.dictationProfiles.find(({ id }) => id === legacyProfile.id);
      expect(migratedProfile).toMatchObject({
        id: legacyProfile.id,
        name: legacyProfile.name,
        processingMode: legacyProfile.processingMode,
        smartPrompt: legacyProfile.smartPrompt,
      });
      if (legacyProfile.name !== 'Old Alt X') {
        expect(migratedProfile?.shortcut).toEqual(legacyProfile.shortcut);
      }
    }
    expect(migrated.transcription.language).toBe('ru');
    expect(migrated.dictationProfiles).toHaveLength(13);
    expect(migrated.dictationProfiles.map(({ name }) => name)).toEqual([
      'Existing General',
      'Existing Prompt',
      'Prompt to English',
      'Markdown',
      'Translate to English',
      'Old Alt X',
      'Old Alt T',
      'Custom A',
      'Custom B',
      'Custom C',
      'Custom D',
      'Custom E',
      'Custom F',
    ]);
    expect(migrated.dictationProfiles.find(({ name }) => name === 'Old Alt X')?.shortcut).toEqual({
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['G'],
    });
    expect(migrated.dictationProfiles.find(({ name }) => name === 'Old Alt T')?.shortcut).toEqual({
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['T'],
    });
    expect(migrated.dictationProfiles.find(({ name }) => name === 'Old Alt T')?.smartPrompt).toBe(
      'Keep this custom prompt',
    );
  });

  it('relocates an edited General Alt+X descendant while preserving profile privacy and content', () => {
    const fixture = readRawFixtures('legacy-v21.json')[0];
    if (fixture === undefined) throw new Error('Missing v21 fixture');
    const source = LegacySettingsV21Schema.parse(structuredClone(fixture.source));
    const general = source.dictationProfiles.find(({ id }) => id === 'general');
    if (general === undefined) throw new Error('Missing legacy General profile');
    general.shortcut = {
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['X', 'Q'],
    };
    general.smartPrompt = 'Preserve this edited General prompt';
    const formerAltX = source.dictationProfiles.find(({ name }) => name === 'Old Alt X');
    if (formerAltX === undefined) throw new Error('Missing legacy custom profile');
    formerAltX.shortcut = {
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['Y'],
    };

    const migrated = migrateV21ToCurrent(source);
    expect(migrated.app.defaultProcessingMode).toBe('raw');
    expect(migrated.dictationProfiles.find(({ id }) => id === 'general')).toMatchObject({
      name: 'Existing General',
      shortcut: {
        modifiers: { ctrl: false, alt: true, shift: false, meta: false },
        keys: ['G'],
      },
      processingMode: 'raw',
      smartPrompt: 'Preserve this edited General prompt',
    });
    expect(migrated.dictationProfiles.find(({ name }) => name === 'Old Alt X')?.shortcut).toEqual({
      modifiers: { ctrl: false, alt: true, shift: false, meta: false },
      keys: ['Y'],
    });
  });

  it.each([
    [null, 'auto'],
    ['x', 'auto'],
    ['auto-detect', 'auto'],
    ['Mandarin', 'zh'],
    ['French', 'fr'],
  ] as const)('normalizes the v21 source language %s to %s', (language, expected) => {
    const fixture = readRawFixtures('legacy-v21.json')[0];
    if (fixture === undefined) throw new Error('Missing v21 fixture');
    const source = structuredClone(fixture.source);
    const transcription = readRecord(source.transcription);
    if (transcription === null) throw new Error('Missing transcription settings');
    (transcription as Record<string, unknown>).language = language;

    const migrated = migrateV21ToCurrent(source);
    expect(migrated.transcription.language).toBe(expected);
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
      'legacy-provider-contracts.ts',
      'legacy-text-contracts.ts',
      'legacy-settings-v1-v12.ts',
      'legacy-settings-v13-v18.ts',
      'legacy-settings-v19.ts',
      'legacy-settings-v20.ts',
      'legacy-settings-v21.ts',
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
      const movedFromNewReservedPrefix =
        sourceProfile.id !== 'general' &&
        sourceProfile.activationKey === 'X' &&
        sourceProfile.shift !== true;
      if (!movedFromNewReservedPrefix) {
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
    expect(migrated.recording.preferredMicrophoneId).toBe(
      recording.preferredMicrophoneId === 'default' ? null : recording.preferredMicrophoneId,
    );
  }
  const transcription = readRecord(source.transcription);
  if (typeof transcription?.language === 'string') {
    expect(migrated.transcription.language).toBe(
      normalizeWhisperSourceLanguage(transcription.language),
    );
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
