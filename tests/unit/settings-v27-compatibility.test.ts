import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it, vi } from 'vitest';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { LegacySettingsV27Schema } from '../../app/src/main/persistence/settings-migrations/legacy-settings-v27';
import {
  SettingsStore,
  UnsupportedSettingsVersionError,
} from '../../app/src/main/persistence/settings-store';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
} from '../../app/src/shared/schemas/dictation-profiles';
import {
  DEFAULT_SETTINGS,
  SETTINGS_SCHEMA_VERSION,
  SettingsSchema,
  SettingsV28Schema,
} from '../../app/src/shared/schemas/settings';

const fixturePath = resolve('tests/fixtures/compatibility/settings-v27.json');
const v28FixturePath = resolve('tests/fixtures/compatibility/settings-v28.json');
const historicalFixtures = JSON.parse(
  readFileSync(resolve('tests/fixtures/compatibility/settings-v22-v26.json'), 'utf8'),
) as { readonly name: string; readonly source: Record<string, unknown> }[];

function readFixture(path = fixturePath): Record<string, unknown> {
  return JSON.parse(readFileSync(path, 'utf8')) as Record<string, unknown>;
}

function memoryIo(source: unknown) {
  let persisted = structuredClone(source);
  const write = vi.fn((_path: string, value: unknown) => {
    persisted = structuredClone(value);
    return Promise.resolve();
  });
  return {
    io: {
      read: () => Promise.resolve(JSON.stringify(persisted)),
      write,
      preserveInvalid: () => Promise.resolve(null),
    },
    write,
    persisted: () => structuredClone(persisted),
  };
}

async function simulateFrozenV27Reader(
  source: unknown,
  preserveInvalid: () => Promise<unknown>,
): Promise<void> {
  const version =
    typeof source === 'object' &&
    source !== null &&
    !Array.isArray(source) &&
    'schemaVersion' in source
      ? (source as Readonly<Record<string, unknown>>).schemaVersion
      : undefined;
  if (typeof version === 'number' && version > 27) {
    throw new UnsupportedSettingsVersionError(version);
  }
  const parsed = LegacySettingsV27Schema.safeParse(source);
  if (!parsed.success) await preserveInvalid();
}

describe('conditional settings v27/v28 compatibility', () => {
  it('parses and serializes the frozen fixture without changing any released v27 value', () => {
    const source = readFixture();
    const parsed = LegacySettingsV27Schema.parse(source);

    expect(parsed).toEqual(source);
    expect(JSON.parse(JSON.stringify(parsed))).toEqual(source);
  });

  it('upgrades released v27 in memory but leaves its compatible on-disk bytes untouched', async () => {
    const source = readFixture();
    const state = memoryIo(source);
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });

    await store.initialize();

    expect(SETTINGS_SCHEMA_VERSION).toBe(28);
    expect(SETTINGS_MIGRATIONS[27]).toBeTypeOf('function');
    expect(store.getDiagnostic()).toBeNull();
    expect(state.write).not.toHaveBeenCalled();
    expect(store.get()).toEqual({ ...source, schemaVersion: 28 });
    expect(state.persisted()).toEqual(source);
  });

  it.each(historicalFixtures)(
    'migrates and persists the immutable $name fixture as released-compatible v27',
    async ({ source }) => {
      const state = memoryIo(source);
      const store = new SettingsStore('settings.json', {
        migrations: SETTINGS_MIGRATIONS,
        io: state.io,
      });

      await store.initialize();

      const migrated = store.get();
      expect(store.getDiagnostic()).toBeNull();
      expect(state.write).toHaveBeenCalledOnce();
      expect(migrated.schemaVersion).toBe(28);
      expect(state.persisted()).toMatchObject({ schemaVersion: 27 });
      expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(true);
      const sourceProfiles = source.dictationProfiles as {
        readonly id: string;
        readonly name: string;
        readonly processingMode: 'raw' | 'smart';
        readonly smartPrompt: string | null;
      }[];
      for (const profile of sourceProfiles) {
        expect(migrated.dictationProfiles.find(({ id }) => id === profile.id)).toMatchObject({
          id: profile.id,
          name: profile.name,
          processingMode: profile.processingMode,
          smartPrompt: profile.smartPrompt,
        });
      }
      if (source.schemaVersion === 26) {
        expect(migrated.recording.preferredMicrophoneId).toBeNull();
      }

      state.write.mockClear();
      const restarted = new SettingsStore('settings.json', {
        migrations: SETTINGS_MIGRATIONS,
        io: state.io,
      });
      await restarted.initialize();
      expect(restarted.get()).toEqual(migrated);
      expect(state.write).not.toHaveBeenCalled();
    },
  );

  it('migrates a shared-prefix v27 development file to explicit v28 without losing values', async () => {
    const source = readFixture(v28FixturePath);
    source.schemaVersion = 27;
    expect(LegacySettingsV27Schema.safeParse(source).success).toBe(false);

    const state = memoryIo(source);
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });
    await store.initialize();

    expect(store.getDiagnostic()).toBeNull();
    expect(state.write).toHaveBeenCalledOnce();
    expect(state.persisted()).toEqual({ ...source, schemaVersion: 28 });
    expect(store.get()).toEqual(state.persisted());
  });

  it('persists shared-prefix state as v28 and never silently downgrades it later', async () => {
    const source = readFixture();
    const state = memoryIo(source);
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });
    await store.initialize();

    const sharedProfile = readFixture(v28FixturePath).dictationProfiles;
    if (!Array.isArray(sharedProfile)) throw new Error('Missing v28 profiles');
    await store.update({
      dictationProfiles: SettingsV28Schema.shape.dictationProfiles.parse(sharedProfile),
    });
    expect(state.persisted()).toMatchObject({ schemaVersion: 28 });
    expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(false);

    await store.update({
      dictationProfiles: LegacySettingsV27Schema.parse(source).dictationProfiles,
    });
    expect(state.persisted()).toMatchObject({ schemaVersion: 28 });
    expect(store.get().schemaVersion).toBe(28);
  });

  it('loads frozen v28 directly and a frozen v27 reader rejects it without replacement', async () => {
    const source = readFixture(v28FixturePath);
    expect(SettingsSchema.parse(source)).toEqual(source);
    expect(LegacySettingsV27Schema.safeParse(source).success).toBe(false);

    const state = memoryIo(source);
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });
    await store.initialize();
    expect(state.write).not.toHaveBeenCalled();
    expect(store.get()).toEqual(source);

    const releasedPreserve = vi.fn();
    await expect(simulateFrozenV27Reader(source, releasedPreserve)).rejects.toMatchObject({
      foundVersion: 28,
    });
    expect(releasedPreserve).not.toHaveBeenCalled();
    expect(state.persisted()).toEqual(source);
  });

  it('recovers corrupt arrived-as-v28 settings behind the v28 persistence fence', async () => {
    const source = { schemaVersion: 28, corrupt: true };
    const state = memoryIo(source);
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'schema',
    });
    expect(store.get()).toEqual(DEFAULT_SETTINGS);
    expect(state.write).toHaveBeenCalledOnce();
    expect(state.persisted()).toEqual(DEFAULT_SETTINGS);
    expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(false);

    const releasedPreserve = vi.fn();
    await expect(
      simulateFrozenV27Reader(state.persisted(), releasedPreserve),
    ).rejects.toMatchObject({
      foundVersion: 28,
    });
    expect(releasedPreserve).not.toHaveBeenCalled();
  });

  it('retains the arrived-as-v28 recovery fence across restart', async () => {
    const state = memoryIo({ schemaVersion: 28, corrupt: true });
    const recovering = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });
    await recovering.initialize();
    state.write.mockClear();

    const restarted = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });
    await restarted.initialize();

    expect(restarted.getDiagnostic()).toBeNull();
    expect(restarted.get()).toEqual(DEFAULT_SETTINGS);
    expect(state.persisted()).toEqual(DEFAULT_SETTINGS);
    expect(state.write).not.toHaveBeenCalled();

    await restarted.update({ app: { closeToTray: false } });
    expect(state.persisted()).toMatchObject({ schemaVersion: 28, app: { closeToTray: false } });
    expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(false);
  });

  it('does not downgrade an arrived-as-v28 recovery during abort rollback or later writes', async () => {
    let persisted: unknown = { schemaVersion: 28, corrupt: true };
    let releaseUpdateWrite: (() => void) | undefined;
    const updateWrite = new Promise<void>((resolveWrite) => {
      releaseUpdateWrite = resolveWrite;
    });
    const write = vi.fn((_path: string, value: unknown) => {
      persisted = structuredClone(value);
      return write.mock.calls.length === 2 ? updateWrite : Promise.resolve();
    });
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: {
        read: () => Promise.resolve(JSON.stringify(persisted)),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();

    const controller = new AbortController();
    const update = store.update({ app: { soundsEnabled: false } }, controller.signal);
    await vi.waitFor(() => expect(write).toHaveBeenCalledTimes(2));
    controller.abort();
    releaseUpdateWrite?.();

    await expect(update).rejects.toMatchObject({ name: 'AbortError' });
    expect(persisted).toEqual(DEFAULT_SETTINGS);
    expect(LegacySettingsV27Schema.safeParse(persisted).success).toBe(false);

    await store.update({ app: { closeToTray: false } });
    expect(persisted).toMatchObject({ schemaVersion: 28, app: { closeToTray: false } });
    for (const [, value] of write.mock.calls) {
      expect(value).toMatchObject({ schemaVersion: 28 });
    }
  });

  it.each([
    { name: 'string lookalike', source: { schemaVersion: '28' } },
    { name: 'null', source: { schemaVersion: null } },
    { name: 'array', source: { schemaVersion: [28] } },
    { name: 'object', source: { schemaVersion: { value: 28 } } },
    { name: 'missing', source: { arrivedVersion: 28 } },
  ])(
    'does not infer the v28 recovery fence from a malformed discriminator: $name',
    async ({ source }) => {
      const state = memoryIo(source);
      const store = new SettingsStore('settings.json', {
        migrations: SETTINGS_MIGRATIONS,
        io: state.io,
      });

      await store.initialize();

      expect(store.getDiagnostic()).toMatchObject({
        code: 'INVALID_SETTINGS_RECOVERED',
        reason: 'schema',
      });
      expect(state.persisted()).toMatchObject({ schemaVersion: 27 });
      expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(true);
    },
  );

  it('does not infer the v28 recovery fence from a fractional numeric discriminator', async () => {
    const state = memoryIo({ schemaVersion: 27.5 });
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: state.io,
    });

    await store.initialize();

    expect(store.getDiagnostic()).toMatchObject({
      code: 'INVALID_SETTINGS_RECOVERED',
      reason: 'migration',
    });
    expect(state.persisted()).toMatchObject({ schemaVersion: 27 });
    expect(LegacySettingsV27Schema.safeParse(state.persisted()).success).toBe(true);
  });

  it('rolls an aborted v27-to-v28 transition back to v27 and keeps later writes compatible', async () => {
    const source = readFixture();
    const sharedProfiles = readFixture(v28FixturePath).dictationProfiles;
    if (!Array.isArray(sharedProfiles)) throw new Error('Missing v28 profiles');
    let persisted: unknown = structuredClone(source);
    let releaseFirstWrite: (() => void) | undefined;
    const firstWrite = new Promise<void>((resolveWrite) => {
      releaseFirstWrite = resolveWrite;
    });
    const write = vi.fn((_path: string, value: unknown) => {
      persisted = structuredClone(value);
      return write.mock.calls.length === 1 ? firstWrite : Promise.resolve();
    });
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: {
        read: () => Promise.resolve(JSON.stringify(source)),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();
    const controller = new AbortController();
    const update = store.update(
      { dictationProfiles: SettingsV28Schema.shape.dictationProfiles.parse(sharedProfiles) },
      controller.signal,
    );
    await vi.waitFor(() => expect(write).toHaveBeenCalledOnce());
    controller.abort();
    releaseFirstWrite?.();

    await expect(update).rejects.toMatchObject({ name: 'AbortError' });
    expect(persisted).toEqual(source);
    expect(LegacySettingsV27Schema.safeParse(persisted).success).toBe(true);

    await store.update({ app: { soundsEnabled: true } });
    expect(persisted).toMatchObject({ schemaVersion: 27 });
    expect(LegacySettingsV27Schema.safeParse(persisted).success).toBe(true);
  });

  it('keeps v27 after a failed transition write and a later compatible update', async () => {
    const source = readFixture();
    const sharedProfiles = readFixture(v28FixturePath).dictationProfiles;
    if (!Array.isArray(sharedProfiles)) throw new Error('Missing v28 profiles');
    let persisted: unknown = structuredClone(source);
    const write = vi
      .fn((_path: string, value: unknown) => {
        persisted = structuredClone(value);
        return Promise.resolve();
      })
      .mockRejectedValueOnce(new Error('disk full'));
    const store = new SettingsStore('settings.json', {
      migrations: SETTINGS_MIGRATIONS,
      io: {
        read: () => Promise.resolve(JSON.stringify(source)),
        write,
        preserveInvalid: () => Promise.resolve(null),
      },
    });
    await store.initialize();

    await expect(
      store.update({
        dictationProfiles: SettingsV28Schema.shape.dictationProfiles.parse(sharedProfiles),
      }),
    ).rejects.toThrow('disk full');
    persisted = structuredClone(source);
    await store.update({ app: { soundsEnabled: true } });
    expect(persisted).toMatchObject({ schemaVersion: 27 });
    expect(LegacySettingsV27Schema.safeParse(persisted).success).toBe(true);
  });

  it('snapshots the built-in defaults and explicit current schema edge', () => {
    expect({
      settingsVersion: SETTINGS_SCHEMA_VERSION,
      defaults: [
        DEFAULT_GENERAL_PROFILE,
        DEFAULT_PROMPT_PROFILE,
        DEFAULT_PROMPT_TO_ENGLISH_PROFILE,
        DEFAULT_MARKDOWN_PROFILE,
        DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
      ].map(({ id, shortcut }) => ({ id, shortcut })),
    }).toMatchInlineSnapshot(`
      {
        "defaults": [
          {
            "id": "general",
            "shortcut": {
              "keys": [
                "X",
              ],
              "modifiers": {
                "alt": true,
                "ctrl": false,
                "meta": false,
                "shift": false,
              },
            },
          },
          {
            "id": "prompt",
            "shortcut": {
              "keys": [
                "X",
                "P",
              ],
              "modifiers": {
                "alt": true,
                "ctrl": false,
                "meta": false,
                "shift": false,
              },
            },
          },
          {
            "id": "prompt-to-english",
            "shortcut": {
              "keys": [
                "X",
                "Q",
              ],
              "modifiers": {
                "alt": true,
                "ctrl": false,
                "meta": false,
                "shift": false,
              },
            },
          },
          {
            "id": "markdown",
            "shortcut": {
              "keys": [
                "X",
                "M",
              ],
              "modifiers": {
                "alt": true,
                "ctrl": false,
                "meta": false,
                "shift": false,
              },
            },
          },
          {
            "id": "translate-to-english",
            "shortcut": {
              "keys": [
                "X",
                "T",
              ],
              "modifiers": {
                "alt": true,
                "ctrl": false,
                "meta": false,
                "shift": false,
              },
            },
          },
        ],
        "settingsVersion": 28,
      }
    `);
  });

  it('keeps frozen settings and transfer boundaries independent from mutable shortcut schemas', () => {
    const frozenModules = [
      'legacy-settings-v27',
      'legacy-v27-profiles',
      'legacy-v27-providers',
      'legacy-v27-text',
    ];
    for (const module of frozenModules) {
      const path = `app/src/main/persistence/settings-migrations/${module}.ts`;
      const source = readFileSync(resolve(path), 'utf8');
      const dependencies = Array.from(
        source.matchAll(/from ['"]([^'"]+)['"]/gu),
        (match) => match[1],
      );
      for (const dependency of dependencies) {
        expect(['zod', ...frozenModules.map((name) => `./${name}`)], path).toContain(dependency);
      }
      expect(source, path).not.toMatch(/shared[\\/]schemas/u);
    }

    for (const path of ['app/src/shared/schemas/settings-transfer-v1.ts']) {
      const source = readFileSync(resolve(path), 'utf8');
      expect(
        Array.from(source.matchAll(/^import .*;$/gmu), ([statement]) => statement),
        path,
      ).toEqual(["import { z } from 'zod';"]);
    }

    const transferV2 = readFileSync(
      resolve('app/src/shared/schemas/settings-transfer-v2.ts'),
      'utf8',
    );
    expect(Array.from(transferV2.matchAll(/from ['"]([^'"]+)['"]/gu), (match) => match[1])).toEqual(
      ['zod', './settings-transfer-v1', './settings-transfer-v1'],
    );

    for (const path of [
      'app/src/main/persistence/settings-migrations/transforms.ts',
      'app/src/main/persistence/settings-migrations/legacy-transforms.ts',
      'app/src/main/persistence/settings-migrations/legacy-shortcut-contract.ts',
    ]) {
      const source = readFileSync(resolve(path), 'utf8');
      expect(source, path).not.toMatch(/shared[\\/]schemas[\\/](?:dictation-profiles|shortcut)/u);
    }

    for (const version of [22, 23, 24, 25, 26, 27]) {
      const path = resolve(
        `app/src/main/persistence/settings-migrations/legacy-settings-v${String(version)}.ts`,
      );
      expect(readFileSync(path, 'utf8'), path).not.toMatch(/shared[\\/]schemas/u);
    }
  });
});
