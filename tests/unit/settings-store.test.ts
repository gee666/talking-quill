import { readdir, readFile, writeFile } from 'node:fs/promises';
import { dirname, join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import {
  SettingsStore,
  UnsupportedSettingsVersionError,
  type SettingsStoreIo,
} from '../../app/src/main/persistence/settings-store';
import {
  DEFAULT_SETTINGS,
  PublicSettingsPatchSchema,
  SETTINGS_SCHEMA_VERSION,
  SettingsPatchSchema,
  SettingsSchema,
} from '../../app/src/shared/schemas/settings';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const directories: string[] = [];
const LEGACY_PROVIDER_ENDPOINTS = [
  'file:///legacy/provider',
  'ftp://legacy.example/models',
] as const;
const LEGACY_V14_PROVIDER_IDS = [
  'openai',
  'generic-openai',
  'lmstudio',
  'localai',
  'koboldcpp',
  'textgenwebui',
  'docker-model-runner',
  'lemonade',
  'foundry',
  'omlx',
  'groq',
  'openrouter',
  'togetherai',
  'fireworksai',
  'deepseek',
  'perplexity',
  'mistral',
  'novita',
  'cometapi',
  'ppio',
  'apipie',
  'sambanova',
  'cerebras',
  'giteeai',
  'minimax',
  'moonshotai',
  'zai',
  'xai',
  'nvidia-nim',
  'privatemode',
  'litellm',
  'ollama',
  'anthropic',
  'gemini',
  'azure',
  'bedrock',
  'cohere',
] as const;
afterEach(async () => {
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

async function testPath() {
  const directory = await createTestDirectory('settings');
  directories.push(directory);
  return join(directory, 'settings.json');
}

function validSettings(
  overrides: { readonly enabled?: boolean; readonly closeToTray?: boolean } = {},
) {
  return JSON.stringify({
    ...structuredClone(DEFAULT_SETTINGS),
    app: {
      ...structuredClone(DEFAULT_SETTINGS.app),
      enabled: overrides.enabled ?? true,
      closeToTray: overrides.closeToTray ?? true,
    },
    recording: { preferredMicrophoneId: null, silencePreset: 'average' },
  });
}

describe('SettingsStore', () => {
  it('migrates v17 by removing only the retired Pi extension preference', () => {
    const legacy = {
      ...structuredClone(DEFAULT_SETTINGS),
      schemaVersion: 17,
      smartProcessing: {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        selectedProviderId: 'pi' as const,
        providers: { pi: { modelId: 'future/model', thinking: 'high' as const } },
        piInstallationPath: 'C:\\Tools\\Pi.CMD',
        piExtensionsEnabled: false,
      },
    };
    const migrated = SettingsSchema.parse(SETTINGS_MIGRATIONS[17]?.(legacy));
    expect(migrated).toMatchObject({
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      smartProcessing: {
        selectedProviderId: 'pi',
        providers: { pi: { modelId: 'future/model', thinking: 'high' } },
        piInstallationPath: 'C:\\Tools\\Pi.CMD',
      },
    });
    expect(migrated.smartProcessing).not.toHaveProperty('piExtensionsEnabled');
  });

  it('migrates the removed large model to turbo and invalidates its stale readiness evidence', () => {
    const legacy = {
      ...structuredClone(DEFAULT_SETTINGS),
      schemaVersion: 18,
      transcription: { modelId: 'Xenova/whisper-large', language: 'ru' },
      welcome: {
        ...structuredClone(DEFAULT_SETTINGS.welcome),
        modelEvidence: {
          modelId: 'Xenova/whisper-large',
          manifestRevision: 'a'.repeat(40),
          verified: true,
          verifiedAt: 1,
        },
      },
    };
    const migrated = SettingsSchema.parse(SETTINGS_MIGRATIONS[18]?.(legacy));
    expect(migrated.transcription).toEqual({
      modelId: 'onnx-community/whisper-large-v3-turbo',
      language: 'ru',
    });
    expect(migrated.welcome.modelEvidence).toBeNull();
    expect(migrated.dictationProfiles).toEqual([
      {
        id: 'general',
        name: 'General',
        activationKey: 'Z',
        shift: false,
        processingMode: 'raw',
        smartPrompt: null,
      },
      {
        id: 'prompt',
        name: 'Prompt',
        activationKey: 'Z',
        shift: true,
        processingMode: 'smart',
        smartPrompt:
          'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
      },
    ]);
  });

  it('migrates exact legacy General key/mode mirrors for default and non-default values', () => {
    for (const [activationKey, defaultProcessingMode] of [
      ['Z', 'raw'],
      ['Q', 'smart'],
    ] as const) {
      const legacy = {
        ...structuredClone(DEFAULT_SETTINGS),
        schemaVersion: 18,
        app: {
          ...structuredClone(DEFAULT_SETTINGS.app),
          activationKey,
          defaultProcessingMode,
        },
        welcome: {
          ...structuredClone(DEFAULT_SETTINGS.welcome),
          activationTested: true,
          activationEvidence: {
            activationKey,
            enabled: true,
            helperProtocol: 2,
            readinessGeneration: 1,
            observedAt: 1,
          },
        },
      };
      const migrated = SettingsSchema.parse(SETTINGS_MIGRATIONS[18]?.(legacy));
      expect(migrated.dictationProfiles.find((profile) => profile.id === 'general')).toEqual({
        id: 'general',
        name: 'General',
        activationKey,
        shift: false,
        processingMode: defaultProcessingMode,
        smartPrompt: null,
      });
      expect(migrated.app).toMatchObject({ activationKey, defaultProcessingMode });
      expect(migrated.welcome).toMatchObject({
        activationTested: false,
        activationEvidence: null,
      });
    }
  });

  it('advances one monotonic Smart revision for every committed Smart dimension only', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    const configs = new ProviderConfigService(store);
    const revisions: number[] = [];
    configs.subscribeSmartRevision((revision) => revisions.push(revision));

    await store.update({ app: { soundsEnabled: false } });
    expect(configs.smartRevision()).toBe(0);
    await store.update({
      smartProcessing: {
        providerReplacements: {
          ollama: { baseUrl: 'http://127.0.0.1:11434', modelId: 'qwen3:4b', keepAlive: 60 },
        },
      },
    });
    await store.update({ smartProcessing: { credentialEpochs: { ollama: 1 } } });
    await store.update({ smartProcessing: { onScreenAwarenessEnabled: true } });
    await store.update({
      smartProcessing: {
        visionOverrides: [
          {
            providerId: 'generic-openai',
            binding: 'http://127.0.0.1:8080/v1',
            modelId: 'private-vision',
            verifiedAt: 1,
          },
        ],
      },
    });
    await store.update({
      customVocabulary: [
        {
          id: '11111111-1111-4111-8111-111111111111',
          value: 'Acme',
          createdAt: 1,
          updatedAt: 1,
        },
      ],
    });

    expect(revisions).toEqual([1, 2, 3, 4, 5]);
    expect(configs.smartRevision()).toBe(5);
  });

  it('creates strict defaults and persists updates across restart', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    expect(store.get()).toEqual(DEFAULT_SETTINGS);

    await Promise.all([
      store.update({ app: { enabled: false } }),
      store.update({ app: { closeToTray: false } }),
      store.update({
        recording: { preferredMicrophoneId: 'studio-mic', silencePreset: 'relaxed' },
      }),
    ]);
    await store.flush();

    const restarted = new SettingsStore(path);
    await restarted.initialize();
    expect(restarted.get().app).toMatchObject({ enabled: false, closeToTray: false });
    expect(restarted.get().recording).toEqual({
      preferredMicrophoneId: 'studio-mic',
      silencePreset: 'relaxed',
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(restarted.get());
  });

  it('sanitizes parse corruption and recovers without retaining plaintext canaries', async () => {
    const path = await testPath();
    const canary = 'unparseable-plaintext-secret-canary';
    await writeFile(path, `{not-json"apiKey":"${canary}"`, 'utf8');
    const store = new SettingsStore(path);
    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'parse',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
    const userDataFiles = await readdir(dirname(path));
    expect(userDataFiles.some((name) => name.endsWith('.invalid'))).toBe(true);
    for (const name of userDataFiles) {
      expect(await readFile(join(dirname(path), name), 'utf8')).not.toContain(canary);
    }
  });

  it('passes the startup read to corruption preservation without a second source read', async () => {
    const source = '{broken-settings';
    const read = vi.fn<SettingsStoreIo['read']>().mockResolvedValue(source);
    const preserveInvalid = vi
      .fn<SettingsStoreIo['preserveInvalid']>()
      .mockResolvedValue('settings.invalid');
    const store = new SettingsStore('settings.json', {
      io: { read, write: () => Promise.resolve(), preserveInvalid },
    });

    await store.initialize();

    expect(read).toHaveBeenCalledOnce();
    expect(preserveInvalid).toHaveBeenCalledWith('settings.json', source);
  });

  it('runs explicit migration hooks before validation', async () => {
    const path = await testPath();
    await writeFile(path, JSON.stringify({ schemaVersion: 0, active: false }), 'utf8');
    const store = new SettingsStore(path, {
      migrations: {
        0: (input) => ({
          ...structuredClone(DEFAULT_SETTINGS),
          app: {
            ...structuredClone(DEFAULT_SETTINGS.app),
            enabled: input.active,
            closeToTray: true,
          },
        }),
      },
    });
    await store.initialize();
    expect(store.get()).toEqual({
      ...structuredClone(DEFAULT_SETTINGS),
      app: { ...structuredClone(DEFAULT_SETTINGS.app), enabled: false, closeToTray: true },
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(store.get());
  });

  it.each([7, 8, 9] as const)(
    'migrates schema-v%s byte-oversized Task 7-9 data without resetting unrelated settings',
    async (version) => {
      const path = await testPath();
      const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
      legacy.schemaVersion = version;
      legacy.app = { ...structuredClone(DEFAULT_SETTINGS.app), enabled: false };
      delete (legacy.privacy as Record<string, unknown>).retainSmartScreenshots;
      delete (legacy.smartProcessing as Record<string, unknown>).onScreenAwarenessEnabled;
      delete (legacy.smartProcessing as Record<string, unknown>).visionOverrides;
      legacy.voiceCommands = [
        {
          id: '11111111-1111-4111-8111-111111111111',
          trigger: 'legacy oversized',
          snippet: '界'.repeat(70_000),
          createdAt: 1,
          updatedAt: 1,
        },
        {
          id: '22222222-2222-4222-8222-222222222222',
          trigger: 'retained',
          snippet: 'safe',
          createdAt: 2,
          updatedAt: 2,
        },
      ];
      legacy.customVocabulary = [
        {
          id: '33333333-3333-4333-8333-333333333333',
          value: '界'.repeat(150),
          createdAt: 1,
          updatedAt: 1,
        },
        {
          id: '44444444-4444-4444-8444-444444444444',
          value: 'retained',
          createdAt: 2,
          updatedAt: 2,
        },
      ];
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
      await store.initialize();
      expect(store.get().schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
      expect(store.get().app.enabled).toBe(false);
      expect(store.get().voiceCommands.map((command) => command.trigger)).toEqual(['retained']);
      expect(store.get().customVocabulary.map((entry) => entry.value)).toEqual(['retained']);
      expect(store.getDiagnostic()).toBeNull();
    },
  );

  it('migrates v13 settings with diagnostic logging safely disabled', async () => {
    const path = await testPath();
    const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    legacy.schemaVersion = 13;
    delete (legacy.privacy as Record<string, unknown>).diagnosticLoggingEnabled;
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await store.initialize();
    expect(store.get().schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(store.get().privacy.diagnosticLoggingEnabled).toBe(false);
    expect(store.getDiagnostic()).toBeNull();
  });

  it.each(LEGACY_V14_PROVIDER_IDS)(
    'migrates historical v14 provider selection %s without reinterpretation',
    async (providerId) => {
      const path = await testPath();
      const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
      legacy.schemaVersion = 14;
      (legacy.smartProcessing as Record<string, unknown>).selectedProviderId = providerId;
      (legacy.smartProcessing as Record<string, unknown>).providers = {};
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
      await store.initialize();
      expect(store.get().smartProcessing.selectedProviderId).toBe(providerId);
      expect(store.get().schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
      expect(store.getDiagnostic()).toBeNull();
    },
  );

  it('does not accept Pi as a fabricated historical v14 provider', async () => {
    const path = await testPath();
    const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    legacy.schemaVersion = 14;
    (legacy.smartProcessing as Record<string, unknown>).selectedProviderId = 'pi';
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await store.initialize();
    expect(store.get().smartProcessing.selectedProviderId).toBe(
      DEFAULT_SETTINGS.smartProcessing.selectedProviderId,
    );
    expect(store.getDiagnostic()).toMatchObject({ reason: 'migration' });
  });

  it.each(LEGACY_PROVIDER_ENDPOINTS)(
    'preserves an inert v14 provider endpoint and unrelated settings until repaired: %s',
    async (baseUrl) => {
      const path = await testPath();
      const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
      legacy.schemaVersion = 14;
      legacy.app = {
        ...structuredClone(DEFAULT_SETTINGS.app),
        enabled: false,
        soundsEnabled: false,
      };
      legacy.privacy = {
        ...structuredClone(DEFAULT_SETTINGS.privacy),
        historyEnabled: false,
        retainSmartScreenshots: true,
      };
      legacy.transcription = {
        ...structuredClone(DEFAULT_SETTINGS.transcription),
        language: 'x',
      };
      legacy.smartProcessing = {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        selectedProviderId: 'generic-openai',
        providers: {
          ...structuredClone(DEFAULT_SETTINGS.smartProcessing.providers),
          'generic-openai': { baseUrl, modelId: 'legacy-model' },
        },
      };
      legacy.customVocabulary = [
        {
          id: '11111111-1111-4111-8111-111111111111',
          value: 'Retained vocabulary',
          createdAt: 1,
          updatedAt: 1,
        },
      ];
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

      await store.initialize();

      expect(store.getDiagnostic()).toBeNull();
      expect(store.get()).toMatchObject({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        app: { enabled: false, soundsEnabled: false },
        privacy: { historyEnabled: false, retainSmartScreenshots: true },
        transcription: { language: 'x' },
        smartProcessing: {
          selectedProviderId: 'generic-openai',
          providers: { 'generic-openai': { baseUrl, modelId: 'legacy-model' } },
        },
        customVocabulary: [{ value: 'Retained vocabulary' }],
      });
      const configs = new ProviderConfigService(store);
      expect(() => configs.get('generic-openai')).toThrow();

      await store.update({ app: { launchAtLogin: true } });
      expect(store.get().smartProcessing.providers['generic-openai']?.baseUrl).toBe(baseUrl);
      await configs.save({
        providerId: 'generic-openai',
        baseUrl: 'https://api.example.test/v1',
        modelId: 'replacement-model',
      });

      const restarted = new SettingsStore(path);
      await restarted.initialize();
      expect(restarted.getDiagnostic()).toBeNull();
      expect(restarted.get()).toMatchObject({
        app: { enabled: false, soundsEnabled: false, launchAtLogin: true },
        privacy: { historyEnabled: false, retainSmartScreenshots: true },
        transcription: { language: 'x' },
        smartProcessing: {
          providers: {
            'generic-openai': {
              baseUrl: 'https://api.example.test/v1',
              modelId: 'replacement-model',
            },
          },
        },
        customVocabulary: [{ value: 'Retained vocabulary' }],
      });
    },
  );

  it.each(LEGACY_PROVIDER_ENDPOINTS)(
    'loads a current v19 legacy endpoint and permits unrelated updates without making it runnable: %s',
    async (baseUrl) => {
      const path = await testPath();
      const current = structuredClone(DEFAULT_SETTINGS);
      current.app.enabled = false;
      current.privacy.historyEnabled = false;
      current.smartProcessing.selectedProviderId = 'generic-openai';
      current.smartProcessing.providers['generic-openai'] = {
        baseUrl,
        modelId: 'legacy-model',
      };
      await writeFile(path, JSON.stringify(current), 'utf8');
      const store = new SettingsStore(path);

      await store.initialize();

      expect(store.getDiagnostic()).toBeNull();
      expect(() => new ProviderConfigService(store).get('generic-openai')).toThrow();
      await store.update({ app: { closeToTray: false } });
      expect(store.get()).toMatchObject({
        app: { enabled: false, closeToTray: false },
        privacy: { historyEnabled: false },
        smartProcessing: {
          providers: { 'generic-openai': { baseUrl, modelId: 'legacy-model' } },
        },
      });
    },
  );

  it.each(LEGACY_PROVIDER_ENDPOINTS)(
    'keeps persisted legacy endpoint parsing separate from strict provider mutations: %s',
    (baseUrl) => {
      const persisted = structuredClone(DEFAULT_SETTINGS);
      persisted.smartProcessing.selectedProviderId = 'generic-openai';
      persisted.smartProcessing.providers['generic-openai'] = {
        baseUrl,
        modelId: 'legacy-model',
      };

      expect(SettingsSchema.safeParse(persisted).success).toBe(true);
      for (const field of ['providers', 'providerReplacements'] as const) {
        expect(
          SettingsPatchSchema.safeParse({
            smartProcessing: {
              [field]: {
                'generic-openai': { baseUrl, modelId: 'legacy-model' },
              },
            },
          }).success,
        ).toBe(false);
      }
      expect(
        SettingsPatchSchema.safeParse({
          smartProcessing: {
            providerReplacements: {
              'generic-openai': {
                baseUrl: 'https://repaired.example.test/v1',
                modelId: 'legacy-model',
              },
            },
          },
        }).success,
      ).toBe(true);
    },
  );

  it.each([13, 15, 16, 17, 18] as const)(
    'preserves a legacy provider endpoint through the v%s current-shape migration',
    async (version) => {
      const path = await testPath();
      const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
      legacy.schemaVersion = version;
      legacy.app = { ...structuredClone(DEFAULT_SETTINGS.app), enabled: false };
      legacy.smartProcessing = {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        selectedProviderId: 'generic-openai',
        providers: {
          'generic-openai': {
            baseUrl: 'ftp://legacy.example/models',
            modelId: 'legacy-model',
          },
        },
        ...(version === 17 ? { piExtensionsEnabled: false } : {}),
      };
      if (version === 13) {
        delete (legacy.privacy as Record<string, unknown>).diagnosticLoggingEnabled;
      }
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

      await store.initialize();

      expect(store.getDiagnostic()).toBeNull();
      expect(store.get()).toMatchObject({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        app: { enabled: false },
        smartProcessing: {
          selectedProviderId: 'generic-openai',
          providers: {
            'generic-openai': {
              baseUrl: 'ftp://legacy.example/models',
              modelId: 'legacy-model',
            },
          },
        },
      });
      expect(() => new ProviderConfigService(store).get('generic-openai')).toThrow();
    },
  );

  it('migrates v14 settings without losing provider state', async () => {
    const path = await testPath();
    const legacy = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    legacy.schemaVersion = 14;
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await store.initialize();
    expect(store.get().schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(store.get().smartProcessing).toEqual(DEFAULT_SETTINGS.smartProcessing);
    expect(store.getDiagnostic()).toBeNull();
  });

  it('recovers a migration that does not advance its schema version as corruption', async () => {
    const path = await testPath();
    await writeFile(path, JSON.stringify({ schemaVersion: 0 }), 'utf8');
    const store = new SettingsStore(path, { migrations: { 0: (input) => input } });
    await store.initialize();
    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
  });

  it('rejects a future version without renaming or replacing it', async () => {
    const path = await testPath();
    const futureVersion = SETTINGS_SCHEMA_VERSION + 1;
    const future = JSON.stringify({
      schemaVersion: futureVersion,
      app: { enabled: false, closeToTray: false },
      futureField: true,
    });
    await writeFile(path, future, 'utf8');
    const store = new SettingsStore(path);

    await expect(store.initialize()).rejects.toBeInstanceOf(UnsupportedSettingsVersionError);
    expect(store.getDiagnostic()).toEqual({
      code: 'UNSUPPORTED_SETTINGS_VERSION',
      foundVersion: futureVersion,
    });
    expect(await readFile(path, 'utf8')).toBe(future);
    expect((await readdir(dirname(path))).some((name) => name.endsWith('.invalid'))).toBe(false);
  });

  it('does not quarantine valid content when a migrated write fails', async () => {
    const preserveInvalid = vi.fn<SettingsStoreIo['preserveInvalid']>();
    const write = vi.fn<SettingsStoreIo['write']>().mockRejectedValue(new Error('disk full'));
    const io: SettingsStoreIo = {
      read: () => Promise.resolve(JSON.stringify({ schemaVersion: 0, active: false })),
      write,
      preserveInvalid,
    };
    const store = new SettingsStore('settings.json', {
      migrations: {
        0: (input) => ({
          ...structuredClone(DEFAULT_SETTINGS),
          app: {
            ...structuredClone(DEFAULT_SETTINGS.app),
            enabled: input.active,
            closeToTray: true,
          },
        }),
      },
      io,
    });

    await expect(store.initialize()).rejects.toThrow('disk full');
    expect(preserveInvalid).not.toHaveBeenCalled();
    expect(store.getDiagnostic()).toEqual({ code: 'SETTINGS_IO_ERROR', operation: 'write' });
  });

  it('propagates read failures without recovery and clears the diagnostic after a retry', async () => {
    const preserveInvalid = vi.fn<SettingsStoreIo['preserveInvalid']>();
    const write = vi.fn<SettingsStoreIo['write']>();
    const read = vi
      .fn<SettingsStoreIo['read']>()
      .mockRejectedValueOnce(new Error('read denied'))
      .mockResolvedValue(validSettings());
    const store = new SettingsStore('settings.json', {
      io: { read, write, preserveInvalid },
    });

    await expect(store.initialize()).rejects.toThrow('read denied');
    expect(write).not.toHaveBeenCalled();
    expect(preserveInvalid).not.toHaveBeenCalled();
    expect(store.getDiagnostic()).toEqual({ code: 'SETTINGS_IO_ERROR', operation: 'read' });

    await store.initialize();
    expect(store.getDiagnostic()).toBeNull();
  });

  it('fails safely when corrupt-file preservation fails', async () => {
    const write = vi.fn<SettingsStoreIo['write']>();
    const store = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve('{broken'),
        write,
        preserveInvalid: () => Promise.reject(new Error('rename denied')),
      },
    });

    await expect(store.initialize()).rejects.toThrow('rename denied');
    expect(write).not.toHaveBeenCalled();
    expect(store.getDiagnostic()).toEqual({
      code: 'SETTINGS_IO_ERROR',
      operation: 'preserve-invalid',
    });
  });

  it('rejects update writes without changing authoritative state or preserving the file', async () => {
    const preserveInvalid = vi.fn<SettingsStoreIo['preserveInvalid']>();
    const write = vi.fn<SettingsStoreIo['write']>().mockRejectedValue(new Error('write denied'));
    const store = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve(validSettings()),
        write,
        preserveInvalid,
      },
    });
    await store.initialize();

    await expect(store.update({ app: { enabled: false } })).rejects.toThrow('write denied');
    expect(store.get().app.enabled).toBe(true);
    expect(preserveInvalid).not.toHaveBeenCalled();
    expect(store.getDiagnostic()).toEqual({ code: 'SETTINGS_IO_ERROR', operation: 'write' });
    await expect(store.flush()).rejects.toThrow('write denied');
    await expect(store.flush()).resolves.toBeUndefined();
  });

  it('rolls back an abort-aware update cancelled during its atomic write', async () => {
    const firstWrite = deferred<undefined>();
    const values: unknown[] = [];
    const write = vi.fn<SettingsStoreIo['write']>((_path, value) => {
      values.push(structuredClone(value));
      return values.length === 1 ? firstWrite.promise : Promise.resolve();
    });
    const store = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve(validSettings()),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();
    const controller = new AbortController();
    const update = store.update({ app: { enabled: false } }, controller.signal);
    await vi.waitFor(() => expect(write).toHaveBeenCalledOnce());
    controller.abort();
    firstWrite.resolve(undefined);

    await expect(update).rejects.toMatchObject({ name: 'AbortError' });
    expect(store.get().app.enabled).toBe(true);
    expect(values).toHaveLength(2);
    expect(values[0]).toMatchObject({ app: { enabled: false } });
    expect(values[1]).toMatchObject({ app: { enabled: true } });
    expect(store.getDiagnostic()).toBeNull();
    await expect(store.flush()).resolves.toBeUndefined();
  });

  it('keeps flush pending through queued writes and reports a pending write failure', async () => {
    const writes = [deferred<undefined>(), deferred<undefined>()] as const;
    const write = vi
      .fn<SettingsStoreIo['write']>()
      .mockImplementationOnce(() => writes[0].promise)
      .mockImplementationOnce(() => writes[1].promise);
    const store = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve(validSettings()),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();

    const first = store.update({ app: { enabled: false } });
    const flushing = expect(store.flush()).rejects.toThrow('disk removed');
    const second = store.update({ app: { closeToTray: false } });
    await vi.waitFor(() => expect(write).toHaveBeenCalledTimes(1));
    writes[0].resolve(undefined);
    await first;
    await vi.waitFor(() => expect(write).toHaveBeenCalledTimes(2));
    writes[1].reject(new Error('disk removed'));

    await expect(second).rejects.toThrow('disk removed');
    await flushing;
    expect(store.get().app).toMatchObject({ enabled: false, closeToTray: true });
  });

  it('isolates listener failures after a committed write and clears recovered I/O diagnostics', async () => {
    const write = vi
      .fn<SettingsStoreIo['write']>()
      .mockRejectedValueOnce(new Error('temporary failure'))
      .mockResolvedValue(undefined);
    const store = new SettingsStore('settings.json', {
      io: {
        read: () => Promise.resolve(validSettings()),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();
    const notified = vi.fn();
    store.subscribe(() => {
      throw new Error('renderer disappeared');
    });
    store.subscribe(notified);

    await expect(store.update({ app: { enabled: false } })).rejects.toThrow('temporary failure');
    expect(store.getDiagnostic()).toEqual({ code: 'SETTINGS_IO_ERROR', operation: 'write' });
    await expect(store.update({ app: { enabled: false } })).resolves.toMatchObject({
      app: { enabled: false },
    });

    expect(store.getDiagnostic()).toBeNull();
    expect(notified).toHaveBeenCalledOnce();
  });

  it('strictly migrates version 1 settings without losing application preferences', async () => {
    const path = await testPath();
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        app: { enabled: false, closeToTray: false },
      }),
      'utf8',
    );
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toEqual({
      ...structuredClone(DEFAULT_SETTINGS),
      app: { ...structuredClone(DEFAULT_SETTINGS.app), enabled: false, closeToTray: false },
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(store.get());
  });

  it.each([
    {
      name: 'Task 4 recording shape',
      legacy: {
        schemaVersion: 2,
        app: { enabled: false, closeToTray: true },
        recording: { preferredMicrophoneId: 'legacy-mic', silencePreset: 'relaxed' },
      },
      expectedRecording: { preferredMicrophoneId: 'legacy-mic', silencePreset: 'relaxed' },
      expectedProvider: DEFAULT_SETTINGS.smartProcessing,
    },
    {
      name: 'Task 9 provider shape',
      legacy: {
        schemaVersion: 2,
        app: { enabled: true, closeToTray: false },
        smartProcessing: {
          selectedProviderId: 'generic-openai',
          providers: {
            'generic-openai': { baseUrl: 'http://127.0.0.1:8080/v1', modelId: 'legacy-model' },
          },
        },
      },
      expectedRecording: DEFAULT_SETTINGS.recording,
      expectedProvider: {
        selectedProviderId: 'generic-openai',
        providers: {
          'generic-openai': { baseUrl: 'http://127.0.0.1:8080/v1', modelId: 'legacy-model' },
        },
      },
    },
    {
      name: 'Task 9 Bedrock draft without region',
      legacy: {
        schemaVersion: 2,
        app: { enabled: true, closeToTray: true },
        smartProcessing: {
          selectedProviderId: 'bedrock',
          providers: { bedrock: { modelId: 'legacy-profile' } },
        },
      },
      expectedRecording: DEFAULT_SETTINGS.recording,
      expectedProvider: {
        selectedProviderId: 'bedrock',
        providers: { bedrock: { modelId: 'legacy-profile', region: 'us-west-2' } },
      },
    },
    {
      name: 'defensively combined shape',
      legacy: {
        schemaVersion: 2,
        app: { enabled: false, closeToTray: false },
        recording: { preferredMicrophoneId: 'combined-mic', silencePreset: 'aggressive' },
        smartProcessing: {
          selectedProviderId: 'ollama',
          providers: { ollama: { baseUrl: 'http://127.0.0.1:11435', keepAlive: 600 } },
        },
      },
      expectedRecording: {
        preferredMicrophoneId: 'combined-mic',
        silencePreset: 'aggressive',
      },
      expectedProvider: {
        selectedProviderId: 'ollama',
        providers: { ollama: { baseUrl: 'http://127.0.0.1:11435', keepAlive: 600 } },
      },
    },
  ])('migrates $name v2 settings to v5 without losing values', async (fixture) => {
    const path = await testPath();
    await writeFile(path, JSON.stringify(fixture.legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toMatchObject({
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      app: { ...structuredClone(DEFAULT_SETTINGS.app), ...fixture.legacy.app },
      recording: fixture.expectedRecording,
      transcription: DEFAULT_SETTINGS.transcription,
      smartProcessing: fixture.expectedProvider,
    });
  });

  it.each(LEGACY_PROVIDER_ENDPOINTS)(
    'migrates a v2 legacy provider endpoint without resetting application preferences: %s',
    async (baseUrl) => {
      const path = await testPath();
      await writeFile(
        path,
        JSON.stringify({
          schemaVersion: 2,
          app: { enabled: false, closeToTray: false },
          smartProcessing: {
            selectedProviderId: 'generic-openai',
            providers: { 'generic-openai': { baseUrl, modelId: 'legacy-model' } },
          },
        }),
        'utf8',
      );
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

      await store.initialize();

      expect(store.getDiagnostic()).toBeNull();
      expect(store.get()).toMatchObject({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        app: { enabled: false, closeToTray: false },
        smartProcessing: {
          selectedProviderId: 'generic-openai',
          providers: { 'generic-openai': { baseUrl, modelId: 'legacy-model' } },
        },
      });
      expect(() => new ProviderConfigService(store).get('generic-openai')).toThrow();
    },
  );

  it('migrates v3 Bedrock drafts with a safe default region without losing settings', async () => {
    const path = await testPath();
    const legacy = {
      schemaVersion: 3,
      app: { enabled: false, closeToTray: false },
      recording: { preferredMicrophoneId: 'legacy-mic', silencePreset: 'relaxed' },
      smartProcessing: {
        selectedProviderId: 'bedrock',
        providers: {
          bedrock: { modelId: 'profile-id', contextWindow: 32_768 },
          openai: { modelId: 'gpt-4.1-nano' },
        },
      },
    };
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toEqual({
      ...legacy,
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      app: { ...structuredClone(DEFAULT_SETTINGS.app), ...legacy.app },
      transcription: structuredClone(DEFAULT_SETTINGS.transcription),
      dictationProfiles: structuredClone(DEFAULT_SETTINGS.dictationProfiles),
      privacy: structuredClone(DEFAULT_SETTINGS.privacy),
      voiceCommands: [],
      customVocabulary: [],
      welcome: structuredClone(DEFAULT_SETTINGS.welcome),
      smartProcessing: {
        ...legacy.smartProcessing,
        providers: {
          ...legacy.smartProcessing.providers,
          bedrock: { ...legacy.smartProcessing.providers.bedrock, region: 'us-west-2' },
        },
        credentialEpochs: {},
        piInstallationPath: null,
        onScreenAwarenessEnabled: false,
        visionOverrides: [],
      },
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(store.get());
  });

  it('preserves an explicit Bedrock region while migrating v3 settings', async () => {
    const path = await testPath();
    const legacy = {
      schemaVersion: 3,
      app: { enabled: true, closeToTray: true },
      recording: structuredClone(DEFAULT_SETTINGS.recording),
      smartProcessing: {
        selectedProviderId: 'bedrock',
        providers: { bedrock: { region: 'eu-west-1', modelId: 'profile-id' } },
      },
    };
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await store.initialize();
    expect(store.get().smartProcessing.providers.bedrock).toEqual({
      region: 'eu-west-1',
      modelId: 'profile-id',
    });
  });

  it.each([
    {
      name: 'Task 6 v4',
      legacy: {
        schemaVersion: 4,
        app: {
          enabled: false,
          closeToTray: false,
          activationKey: 'Q',
          defaultProcessingMode: 'smart',
          widgetSize: 'huge',
          soundsEnabled: false,
        },
        recording: { preferredMicrophoneId: 'task6-mic', silencePreset: 'aggressive' },
        transcription: { modelId: 'Xenova/whisper-large', language: 'fr' },
        smartProcessing: {
          selectedProviderId: 'generic-openai',
          providers: {
            'generic-openai': {
              baseUrl: 'http://127.0.0.1:8080/v1',
              modelId: 'task6-model',
            },
            bedrock: { modelId: 'legacy-profile' },
          },
        },
      },
    },
    {
      name: 'Task 11 v4',
      legacy: {
        schemaVersion: 4,
        app: { enabled: false, closeToTray: true },
        recording: { preferredMicrophoneId: 'task11-mic', silencePreset: 'relaxed' },
        smartProcessing: {
          selectedProviderId: 'azure',
          providers: {
            azure: {
              baseUrl: 'https://example.openai.azure.com',
              modelId: 'deployment-a',
              modelType: 'reasoning',
            },
            bedrock: { modelId: 'profile-id', region: 'eu-west-1' },
          },
        },
      },
    },
    {
      name: 'defensive hybrid v4',
      legacy: {
        schemaVersion: 4,
        app: {
          enabled: true,
          closeToTray: false,
          activationKey: 'X',
          defaultProcessingMode: 'raw',
          widgetSize: 'max',
          soundsEnabled: false,
        },
        recording: { preferredMicrophoneId: null, silencePreset: 'average' },
        transcription: { modelId: 'Xenova/whisper-large', language: 'de' },
        smartProcessing: {
          selectedProviderId: 'azure',
          providers: {
            azure: {
              baseUrl: 'https://hybrid.openai.azure.com',
              modelId: 'deployment-b',
              modelType: 'reasoning',
            },
            bedrock: { modelId: 'hybrid-profile' },
          },
        },
      },
    },
  ])('migrates $name to v6 without losing either topic settings', async ({ legacy }) => {
    const path = await testPath();
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    const migrated = store.get();
    expect(migrated.schemaVersion).toBe(SETTINGS_SCHEMA_VERSION);
    expect(migrated.app).toEqual({ ...structuredClone(DEFAULT_SETTINGS.app), ...legacy.app });
    expect(migrated.recording).toEqual(legacy.recording);
    expect(migrated.transcription).toEqual(
      'transcription' in legacy
        ? {
            ...legacy.transcription,
            modelId:
              legacy.transcription.modelId === 'Xenova/whisper-large'
                ? 'onnx-community/whisper-large-v3-turbo'
                : legacy.transcription.modelId,
          }
        : DEFAULT_SETTINGS.transcription,
    );
    expect(migrated.smartProcessing.selectedProviderId).toBe(
      legacy.smartProcessing.selectedProviderId,
    );
    expect(migrated.smartProcessing.credentialEpochs).toEqual({});
    expect(migrated.smartProcessing.providers.azure).toEqual(
      legacy.smartProcessing.providers.azure,
    );
    expect(migrated.smartProcessing.providers.bedrock).toEqual({
      ...legacy.smartProcessing.providers.bedrock,
      region: legacy.smartProcessing.providers.bedrock.region ?? 'us-west-2',
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(migrated);
  });

  it.each([
    {
      name: 'partial Task 6 app',
      mutate: (settings: Record<string, unknown>) => {
        settings.app = { enabled: true, closeToTray: true, activationKey: 'Z' };
      },
    },
    {
      name: 'partial transcription section',
      mutate: (settings: Record<string, unknown>) => {
        settings.transcription = { modelId: 'Xenova/whisper-small' };
      },
    },
    {
      name: 'invalid Bedrock region',
      mutate: (settings: Record<string, unknown>) => {
        settings.smartProcessing = {
          selectedProviderId: 'bedrock',
          providers: { bedrock: { modelId: 'profile', region: 'us-west-2.invalid' } },
        };
      },
    },
    {
      name: 'Azure model type on another provider',
      mutate: (settings: Record<string, unknown>) => {
        settings.smartProcessing = {
          selectedProviderId: 'openai',
          providers: { openai: { modelType: 'reasoning' } },
        };
      },
    },
  ])('rejects malformed historical v4: $name', async ({ mutate }) => {
    const path = await testPath();
    const malformed = structuredClone(DEFAULT_SETTINGS) as unknown as Record<string, unknown>;
    malformed.schemaVersion = 4;
    mutate(malformed);
    await writeFile(path, JSON.stringify(malformed), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
  });

  it('migrates v6 settings to v8 with Task 7 and Task 8 defaults', async () => {
    const path = await testPath();
    const legacy: Record<string, unknown> = structuredClone(DEFAULT_SETTINGS);
    legacy.schemaVersion = 6;
    delete legacy.privacy;
    delete legacy.voiceCommands;
    delete legacy.customVocabulary;
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toMatchObject({
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      privacy: DEFAULT_SETTINGS.privacy,
      voiceCommands: [],
      customVocabulary: [],
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(store.get());
  });

  it('migrates v5 settings to the current schema with all defaults and empty credential epochs', async () => {
    const path = await testPath();
    const legacyBase: Record<string, unknown> = structuredClone(DEFAULT_SETTINGS);
    delete legacyBase.privacy;
    delete legacyBase.voiceCommands;
    delete legacyBase.customVocabulary;
    const legacy = {
      ...legacyBase,
      schemaVersion: 5,
      smartProcessing: {
        selectedProviderId: 'generic-openai' as const,
        providers: {
          'generic-openai': { baseUrl: 'http://127.0.0.1:8080/v1', modelId: 'local-model' },
        },
      },
    };
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toMatchObject({
      schemaVersion: SETTINGS_SCHEMA_VERSION,
      privacy: DEFAULT_SETTINGS.privacy,
      voiceCommands: [],
      customVocabulary: [],
      smartProcessing: {
        selectedProviderId: 'generic-openai',
        credentialEpochs: {},
      },
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(store.get());
  });

  it('migrates v6 settings to v8 with privacy defaults without changing provider state', async () => {
    const path = await testPath();
    const legacy: Record<string, unknown> = structuredClone(DEFAULT_SETTINGS);
    Reflect.deleteProperty(legacy, 'privacy');
    Reflect.deleteProperty(legacy, 'voiceCommands');
    Reflect.deleteProperty(legacy, 'customVocabulary');
    await writeFile(path, JSON.stringify({ ...legacy, schemaVersion: 6 }), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.get()).toEqual(DEFAULT_SETTINGS);
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(DEFAULT_SETTINGS);
  });

  it.each([
    { lastStep: 2, completedAt: null },
    { lastStep: 3, completedAt: null },
    { lastStep: 5, completedAt: null },
    { lastStep: 1, completedAt: 1_700_000_000_000 },
  ] as const)(
    'migrates v11 progress at step $lastStep without inventing prerequisite evidence',
    async ({ lastStep, completedAt }) => {
      const path = await testPath();
      const legacy: Record<string, unknown> = structuredClone(DEFAULT_SETTINGS);
      legacy.schemaVersion = 11;
      legacy.app = { ...structuredClone(DEFAULT_SETTINGS.app), launchAtLogin: true };
      legacy.transcription = {
        ...structuredClone(DEFAULT_SETTINGS.transcription),
        language: 'fr',
      };
      legacy.welcome = { lastStep, completedAt };
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

      await store.initialize();

      expect(store.get()).toMatchObject({
        app: { launchAtLogin: true },
        transcription: { language: 'fr' },
        welcome: {
          lastStep: completedAt === null ? 2 : 6,
          completedAt,
          microphoneTested: false,
          activationTested: false,
          microphoneEvidence: null,
          activationEvidence: null,
          modelEvidence: null,
        },
      });
    },
  );

  it('preserves v12 completion without treating booleans as prerequisite evidence', async () => {
    const path = await testPath();
    const legacy = structuredClone(DEFAULT_SETTINGS) as Record<string, unknown>;
    legacy.schemaVersion = 12;
    legacy.welcome = {
      completedAt: 1_700_000_000_000,
      lastStep: 6,
      microphoneTested: true,
      activationTested: true,
    };
    await writeFile(path, JSON.stringify(legacy), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });
    await store.initialize();
    expect(store.get().welcome).toMatchObject({
      lastStep: 6,
      completedAt: 1_700_000_000_000,
      microphoneTested: false,
      activationTested: false,
      microphoneEvidence: null,
      activationEvidence: null,
      modelEvidence: null,
    });
  });

  it.each(['task7', 'task8', 'hybrid'] as const)(
    'migrates %s v7 settings to v8 without losing either topic',
    async (shape) => {
      const path = await testPath();
      const command = {
        id: '11111111-1111-4111-8111-111111111111',
        trigger: 'insert signature',
        snippet: 'Kind regards',
        createdAt: 1_700_000_000_000,
        updatedAt: 1_700_000_000_001,
      };
      const vocabulary = {
        id: '22222222-2222-4222-8222-222222222222',
        value: 'Talking Quill',
        createdAt: 1_700_000_000_002,
        updatedAt: 1_700_000_000_003,
      };
      const legacy: Record<string, unknown> = {
        ...structuredClone(DEFAULT_SETTINGS),
        schemaVersion: 7,
        app: { ...structuredClone(DEFAULT_SETTINGS.app), activationKey: 'Q' },
        recording: { preferredMicrophoneId: 'retained-mic', silencePreset: 'relaxed' },
        transcription: { modelId: 'Xenova/whisper-large', language: 'en' },
        smartProcessing: {
          selectedProviderId: 'azure',
          providers: {
            azure: {
              baseUrl: 'https://retained.openai.azure.com',
              modelId: 'retained-deployment',
              modelType: 'reasoning',
            },
            bedrock: { modelId: 'retained-profile', region: 'eu-west-1' },
          },
          credentialEpochs: { azure: 9, bedrock: 4 },
        },
        privacy: { historyEnabled: false, historyRetentionDays: 30 },
        voiceCommands: [command],
        customVocabulary: [vocabulary],
      };
      if (shape === 'task7') {
        delete legacy.voiceCommands;
        delete legacy.customVocabulary;
      } else if (shape === 'task8') {
        delete legacy.privacy;
      }
      await writeFile(path, JSON.stringify(legacy), 'utf8');
      const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

      await store.initialize();

      const migrated = store.get();
      expect(migrated).toMatchObject({
        schemaVersion: SETTINGS_SCHEMA_VERSION,
        app: legacy.app,
        recording: legacy.recording,
        transcription: {
          modelId: 'onnx-community/whisper-large-v3-turbo',
          language: 'en',
        },
        privacy: shape === 'task8' ? DEFAULT_SETTINGS.privacy : legacy.privacy,
        voiceCommands: shape === 'task7' ? [] : [command],
        customVocabulary: shape === 'task7' ? [] : [vocabulary],
        smartProcessing: legacy.smartProcessing,
      });
      expect(JSON.parse(await readFile(path, 'utf8'))).toEqual(migrated);
    },
  );

  it('fails safe for malformed hybrid v7 settings', async () => {
    const path = await testPath();
    const malformed = {
      ...structuredClone(DEFAULT_SETTINGS),
      schemaVersion: 7,
      voiceCommands: [{ id: '../invalid', trigger: 'go', snippet: 'text' }],
    };
    await writeFile(path, JSON.stringify(malformed), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
  });

  it('rejects malformed v5 credential epoch injection instead of trusting internal state', async () => {
    const path = await testPath();
    const malformed = {
      ...structuredClone(DEFAULT_SETTINGS),
      schemaVersion: 5,
      smartProcessing: {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        credentialEpochs: { openai: 42 },
      },
    };
    await writeFile(path, JSON.stringify(malformed), 'utf8');
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
  });

  it('always derives compatibility mirrors atomically from the General profile', async () => {
    expect(
      SettingsSchema.safeParse({
        ...structuredClone(DEFAULT_SETTINGS),
        app: { ...structuredClone(DEFAULT_SETTINGS.app), activationKey: 'Q' },
      }).success,
    ).toBe(false);
    expect(
      SettingsSchema.safeParse({
        ...structuredClone(DEFAULT_SETTINGS),
        app: { ...structuredClone(DEFAULT_SETTINGS.app), defaultProcessingMode: 'smart' },
      }).success,
    ).toBe(false);

    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    const profiles = store
      .get()
      .dictationProfiles.map((profile) =>
        profile.id === 'general'
          ? { ...profile, activationKey: 'Q' as const, processingMode: 'smart' as const }
          : profile,
      );
    let saved = await store.update({
      app: { activationKey: 'X', defaultProcessingMode: 'raw' },
      dictationProfiles: profiles,
    });
    expect(saved.app).toMatchObject({ activationKey: 'Q', defaultProcessingMode: 'smart' });

    saved = await store.update({ app: { activationKey: 'A', defaultProcessingMode: 'raw' } });
    expect(saved.app).toMatchObject({ activationKey: 'Q', defaultProcessingMode: 'smart' });
  });

  it('preserves activation evidence for unrelated profile CRUD and invalidates its exact binding', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    await store.update({
      welcome: {
        activationTested: true,
        activationEvidence: {
          profileId: 'prompt',
          activationKey: 'Z',
          shift: true,
          enabled: true,
          helperProtocol: 2,
          readinessGeneration: 1,
          observedAt: 1,
        },
      },
    });
    const custom = {
      id: '11111111-1111-4111-8111-111111111111',
      name: 'Untested',
      activationKey: 'Q' as const,
      shift: false,
      processingMode: 'raw' as const,
      smartPrompt: null,
    };
    let saved = await store.update({
      dictationProfiles: [...store.get().dictationProfiles, custom],
    });
    expect(saved.welcome.activationEvidence?.profileId).toBe('prompt');

    saved = await store.update({
      dictationProfiles: saved.dictationProfiles.map((profile) =>
        profile.id === 'general'
          ? { ...profile, name: 'Renamed General', processingMode: 'smart' as const }
          : profile.id === 'prompt'
            ? { ...profile, name: 'Renamed Prompt', smartPrompt: 'Changed preference.' }
            : profile,
      ),
    });
    expect(saved.welcome.activationEvidence?.profileId).toBe('prompt');

    saved = await store.update({
      dictationProfiles: saved.dictationProfiles.filter((profile) => profile.id !== custom.id),
    });
    expect(saved.welcome.activationEvidence?.profileId).toBe('prompt');

    saved = await store.update({
      dictationProfiles: saved.dictationProfiles.map((profile) =>
        profile.id === 'prompt' ? { ...profile, activationKey: 'P' as const } : profile,
      ),
    });
    expect(saved.welcome).toMatchObject({ activationTested: false, activationEvidence: null });
  });

  it('keeps provider configuration out of public settings patches', () => {
    expect(PublicSettingsPatchSchema.safeParse({ app: { activationKey: 'Q' } }).success).toBe(
      false,
    );
    expect(
      PublicSettingsPatchSchema.safeParse({ app: { defaultProcessingMode: 'smart' } }).success,
    ).toBe(false);
    expect(PublicSettingsPatchSchema.safeParse({ transcription: { language: 'en' } }).success).toBe(
      true,
    );
    expect(
      PublicSettingsPatchSchema.safeParse({ smartProcessing: { selectedProviderId: 'openai' } })
        .success,
    ).toBe(false);
    expect(PublicSettingsPatchSchema.safeParse({ providers: {} }).success).toBe(false);
    expect(PublicSettingsPatchSchema.safeParse({ providerReplacements: {} }).success).toBe(false);
  });

  it('rejects malformed v2 settings instead of silently defaulting invalid sections', async () => {
    const path = await testPath();
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 2,
        app: { enabled: true, closeToTray: true },
        recording: { preferredMicrophoneId: null, silencePreset: 'invalid' },
        smartProcessing: structuredClone(DEFAULT_SETTINGS.smartProcessing),
      }),
      'utf8',
    );
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
  });

  it('rejects unknown legacy fields instead of carrying them through migration', async () => {
    const path = await testPath();
    await writeFile(
      path,
      JSON.stringify({
        schemaVersion: 1,
        app: { enabled: true, closeToTray: true },
        apiKey: 'must-not-migrate',
      }),
      'utf8',
    );
    const store = new SettingsStore(path, { migrations: SETTINGS_MIGRATIONS });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(JSON.stringify(store.get())).not.toContain('apiKey');
    expect(await readFile(path, 'utf8')).not.toContain('must-not-migrate');
  });

  it('deep-merges independent provider drafts without dropping sibling fields', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();

    await store.update({
      smartProcessing: {
        providers: { ollama: { modelId: 'llama3.2' } },
      },
    });
    await store.update({
      smartProcessing: {
        selectedProviderId: 'generic-openai',
        providers: {
          'generic-openai': {
            baseUrl: 'http://127.0.0.1:8080/v1',
            contextWindow: 8_192,
          },
        },
      },
    });
    await store.update({
      smartProcessing: {
        providers: { 'generic-openai': { modelId: 'local-model' } },
      },
    });

    expect(store.get().smartProcessing).toEqual({
      selectedProviderId: 'generic-openai',
      credentialEpochs: {},
      piInstallationPath: null,
      onScreenAwarenessEnabled: false,
      visionOverrides: [],
      providers: {
        ollama: {
          baseUrl: 'http://127.0.0.1:11434',
          keepAlive: 300,
          modelId: 'llama3.2',
        },
        'generic-openai': {
          baseUrl: 'http://127.0.0.1:8080/v1',
          contextWindow: 8_192,
          modelId: 'local-model',
        },
      },
    });
  });

  it('persists and explicitly clears the validated Pi installation preference', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    await store.update({
      smartProcessing: { piInstallationPath: 'C:\\Program Files\\npm\\pi.cmd' },
    });
    expect(store.get().smartProcessing.piInstallationPath).toBe('C:\\Program Files\\npm\\pi.cmd');
    await store.update({ smartProcessing: { piInstallationPath: null } });
    expect(store.get().smartProcessing.piInstallationPath).toBeNull();
  });

  it('preserves recording and provider settings across independent updates', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();

    await store.update({
      recording: { preferredMicrophoneId: 'studio-mic', silencePreset: 'relaxed' },
    });
    await store.update({
      smartProcessing: {
        selectedProviderId: 'generic-openai',
        providerReplacements: {
          'generic-openai': { baseUrl: 'http://127.0.0.1:8080/v1', modelId: 'model-a' },
        },
      },
    });
    expect(store.get().recording).toEqual({
      preferredMicrophoneId: 'studio-mic',
      silencePreset: 'relaxed',
    });

    await store.update({ recording: { silencePreset: 'aggressive' } });
    expect(store.get().smartProcessing).toMatchObject({
      selectedProviderId: 'generic-openai',
      providers: {
        'generic-openai': { baseUrl: 'http://127.0.0.1:8080/v1', modelId: 'model-a' },
      },
    });
  });

  it('rejects secret-shaped provider draft fields', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    await expect(
      store.update({
        smartProcessing: {
          providers: { openai: { credential: 'must-not-persist' } },
        },
      } as never),
    ).rejects.toThrow();
    expect(await readFile(path, 'utf8')).not.toContain('must-not-persist');
  });

  it('rejects unknown update fields through complete schema validation', async () => {
    const path = await testPath();
    const store = new SettingsStore(path);
    await store.initialize();
    await expect(
      store.update({ app: { enabled: true, unexpected: true } } as never),
    ).rejects.toThrow();
  });
});

function deferred<Value>() {
  let resolvePromise!: (value: Value | PromiseLike<Value>) => void;
  let rejectPromise!: (reason?: unknown) => void;
  const promise = new Promise<Value>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}
