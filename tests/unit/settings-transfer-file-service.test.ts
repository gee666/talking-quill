import { readFileSync } from 'node:fs';
import { readFile, symlink, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { ProfileActivationCoordinator } from '../../app/src/main/echo/profile-activation-coordinator';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { LegacySettingsV27Schema } from '../../app/src/main/persistence/settings-migrations/legacy-settings-v27';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import {
  DictationProfilesTransferV1Schema,
  DictationProfilesTransferV2Schema,
} from '../../app/src/shared/schemas/settings-transfer';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const showOpenDialog = vi.fn();
const showSaveDialog = vi.fn();
const showMessageBox = vi.fn();
vi.mock('electron', () => ({ dialog: { showOpenDialog, showSaveDialog, showMessageBox } }));

const { SettingsTransferFileService } =
  await import('../../app/src/main/data/settings-transfer-file-service');

const directories: string[] = [];
afterEach(async () => {
  vi.clearAllMocks();
  showMessageBox.mockResolvedValue({ response: 0 });
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

showMessageBox.mockResolvedValue({ response: 0 });

function service(
  platform: string = process.platform,
  profiles = DEFAULT_SETTINGS.dictationProfiles,
) {
  const commands = {
    list: vi.fn(() => DEFAULT_SETTINGS.voiceCommands),
    replace: vi.fn(() => Promise.resolve(DEFAULT_SETTINGS.voiceCommands)),
  };
  const echo = {
    dictationProfiles: profiles,
    replaceProfiles: vi.fn(() => Promise.resolve(DEFAULT_SETTINGS)),
  };
  return {
    commands,
    echo,
    files: new SettingsTransferFileService(commands as never, echo as never, undefined, platform),
  };
}

async function persistedProfileService(settingsPath: string, platform = 'linux') {
  const settings = new SettingsStore(settingsPath, { migrations: SETTINGS_MIGRATIONS });
  await settings.initialize();
  const coordinator = new ProfileActivationCoordinator({
    settings,
    helper: {
      readiness: { status: 'ready' },
      configureActivation: () => Promise.resolve(),
    } as never,
    isModelReady: () => true,
  });
  const echo = {
    get dictationProfiles() {
      return settings.get().dictationProfiles;
    },
    replaceProfiles: coordinator.replaceProfiles.bind(coordinator),
  };
  const commands = {
    list: vi.fn(() => settings.get().voiceCommands),
    replace: vi.fn(() => Promise.resolve(settings.get().voiceCommands)),
  };
  return {
    settings,
    files: new SettingsTransferFileService(commands as never, echo as never, undefined, platform),
  };
}

describe('SettingsTransferFileService', () => {
  it('exports versioned voice-command JSON and imports it transactionally', async () => {
    const directory = await createTestDirectory('command-transfer');
    directories.push(directory);
    const path = join(directory, 'commands.json');
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'my address',
      snippet: '1 Quill Lane',
      createdAt: 1,
      updatedAt: 2,
    };
    const { commands, files } = service();
    commands.list.mockReturnValue([command]);
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: path });

    await expect(files.exportVoiceCommands({} as never)).resolves.toEqual({
      status: 'exported',
      count: 1,
    });
    expect(JSON.parse(await readFile(path, 'utf8'))).toEqual({
      format: 'talking-quill.voice-commands',
      version: 1,
      commands: [command],
    });

    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    await expect(files.importVoiceCommands({} as never)).resolves.toEqual({
      status: 'imported',
      count: 1,
    });
    expect(commands.replace).toHaveBeenCalledWith([command]);
  });

  it('exports v1-compatible profiles as v1 and round-trips them through the file service', async () => {
    const directory = await createTestDirectory('profile-transfer');
    directories.push(directory);
    const path = join(directory, 'profiles.json');
    const { echo, files } = service();
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: path });
    await expect(files.exportDictationProfiles({} as never)).resolves.toEqual({
      status: 'exported',
      count: DEFAULT_SETTINGS.dictationProfiles.length,
    });
    expect(
      DictationProfilesTransferV1Schema.parse(JSON.parse(await readFile(path, 'utf8'))),
    ).toEqual({
      format: 'talking-quill.dictation-profiles',
      version: 1,
      profiles: DEFAULT_SETTINGS.dictationProfiles,
    });

    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    await expect(files.importDictationProfiles({} as never)).resolves.toEqual({
      status: 'imported',
      count: DEFAULT_SETTINGS.dictationProfiles.length,
    });
    expect(echo.replaceProfiles).toHaveBeenCalledWith(DEFAULT_SETTINGS.dictationProfiles);
  });

  it('rejects impossible imported shortcuts and requires confirmation for risky ones', async () => {
    const directory = await createTestDirectory('profile-policy-transfer');
    directories.push(directory);
    const path = join(directory, 'profiles.json');
    const riskyTransfer = {
      format: 'talking-quill.dictation-profiles',
      version: 2,
      profiles: DEFAULT_SETTINGS.dictationProfiles,
    };
    await writeFile(path, JSON.stringify(riskyTransfer), 'utf8');
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    showMessageBox.mockResolvedValueOnce({ response: 1 });
    const risky = service('win32');
    await expect(risky.files.importDictationProfiles({} as never)).resolves.toEqual({
      status: 'cancelled',
    });
    expect(risky.echo.replaceProfiles).not.toHaveBeenCalled();

    const impossibleProfiles = DEFAULT_SETTINGS.dictationProfiles.map((profile, index) =>
      index === 0
        ? {
            ...profile,
            shortcut: {
              modifiers: { ctrl: false, alt: false, shift: false, meta: true },
              keys: ['L'],
            },
          }
        : profile,
    );
    await writeFile(
      path,
      JSON.stringify({ ...riskyTransfer, profiles: impossibleProfiles }),
      'utf8',
    );
    const impossible = service('win32');
    await expect(impossible.files.importDictationProfiles({} as never)).rejects.toThrow('Win + L');
    expect(impossible.echo.replaceProfiles).not.toHaveBeenCalled();
  });

  it('persists, restarts, and downgrades the immutable v1 fixture without value loss', async () => {
    const fixture = JSON.parse(
      readFileSync(resolve('tests/fixtures/compatibility/profile-transfer-v1.json'), 'utf8'),
    ) as unknown;
    const parsed = DictationProfilesTransferV1Schema.parse(fixture);
    const directory = await createTestDirectory('profile-transfer-v1');
    directories.push(directory);
    const importPath = join(directory, 'profiles-v1-import.json');
    const exportPath = join(directory, 'profiles-v1-export.json');
    const settingsPath = join(directory, 'settings.json');
    await writeFile(importPath, JSON.stringify(fixture), 'utf8');
    const importer = await persistedProfileService(settingsPath);
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [importPath] });

    await expect(importer.files.importDictationProfiles({} as never)).resolves.toEqual({
      status: 'imported',
      count: parsed.profiles.length,
    });
    const persisted = JSON.parse(await readFile(settingsPath, 'utf8')) as unknown;
    expect(persisted).toMatchObject({ schemaVersion: 27, dictationProfiles: parsed.profiles });
    expect(LegacySettingsV27Schema.safeParse(persisted).success).toBe(true);
    expect(importer.settings.get().dictationProfiles).toEqual(parsed.profiles);

    const restarted = await persistedProfileService(settingsPath);
    expect(restarted.settings.get().dictationProfiles).toEqual(parsed.profiles);
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: exportPath });
    await expect(restarted.files.exportDictationProfiles({} as never)).resolves.toEqual({
      status: 'exported',
      count: parsed.profiles.length,
    });
    expect(JSON.parse(await readFile(exportPath, 'utf8'))).toEqual(fixture);
  });

  it('imports a v2 envelope with v1-compatible profiles and exports the safe v1 downgrade', async () => {
    const v1 = DictationProfilesTransferV1Schema.parse(
      JSON.parse(
        readFileSync(resolve('tests/fixtures/compatibility/profile-transfer-v1.json'), 'utf8'),
      ) as unknown,
    );
    const directory = await createTestDirectory('profile-transfer-v2-compatible');
    directories.push(directory);
    const importPath = join(directory, 'profiles-v2-compatible-import.json');
    const exportPath = join(directory, 'profiles-v2-compatible-export.json');
    const settingsPath = join(directory, 'settings.json');
    await writeFile(importPath, JSON.stringify({ ...v1, version: 2 }), 'utf8');
    const importer = await persistedProfileService(settingsPath);
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [importPath] });
    await importer.files.importDictationProfiles({} as never);

    showSaveDialog.mockResolvedValue({ canceled: false, filePath: exportPath });
    await importer.files.exportDictationProfiles({} as never);
    expect(
      DictationProfilesTransferV1Schema.parse(JSON.parse(await readFile(exportPath, 'utf8'))),
    ).toMatchObject({ version: 1, profiles: v1.profiles });
  });

  it('persists the immutable v2 fixture as settings v28 and round-trips it unchanged', async () => {
    const fixture = JSON.parse(
      readFileSync(resolve('tests/fixtures/compatibility/profile-transfer-v2.json'), 'utf8'),
    ) as unknown;
    const transfer = DictationProfilesTransferV2Schema.parse(fixture);
    const directory = await createTestDirectory('profile-transfer-v2');
    directories.push(directory);
    const importPath = join(directory, 'profiles-v2-import.json');
    const exportPath = join(directory, 'profiles-v2-export.json');
    const settingsPath = join(directory, 'settings.json');
    await writeFile(importPath, JSON.stringify(transfer), 'utf8');
    const importer = await persistedProfileService(settingsPath);
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [importPath] });

    await expect(importer.files.importDictationProfiles({} as never)).resolves.toEqual({
      status: 'imported',
      count: transfer.profiles.length,
    });
    expect(importer.settings.get().dictationProfiles).toEqual(transfer.profiles);
    expect(JSON.parse(await readFile(settingsPath, 'utf8'))).toMatchObject({ schemaVersion: 28 });

    const restarted = await persistedProfileService(settingsPath);
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: exportPath });
    await expect(restarted.files.exportDictationProfiles({} as never)).resolves.toEqual({
      status: 'exported',
      count: transfer.profiles.length,
    });
    const roundTrip = JSON.parse(await readFile(exportPath, 'utf8')) as unknown;
    expect(DictationProfilesTransferV1Schema.safeParse(roundTrip).success).toBe(false);
    expect(DictationProfilesTransferV2Schema.parse(roundTrip)).toEqual(transfer);
    expect(roundTrip).toEqual(fixture);
  });

  it('rejects malformed files and symbolic links before mutating either domain', async () => {
    const directory = await createTestDirectory('invalid-transfer');
    directories.push(directory);
    const invalid = join(directory, 'invalid.json');
    const target = join(directory, 'target.json');
    const link = join(directory, 'link.json');
    await writeFile(invalid, '{"format":"not-talking-quill"}', 'utf8');
    await writeFile(target, '{}', 'utf8');
    await symlink(target, link);
    const { commands, echo, files } = service();

    showOpenDialog.mockResolvedValueOnce({ canceled: false, filePaths: [invalid] });
    await expect(files.importVoiceCommands({} as never)).rejects.toThrow();
    showOpenDialog.mockResolvedValueOnce({ canceled: false, filePaths: [link] });
    await expect(files.importDictationProfiles({} as never)).rejects.toThrow(
      'valid Talking Quill JSON file',
    );
    expect(commands.replace).not.toHaveBeenCalled();
    expect(echo.replaceProfiles).not.toHaveBeenCalled();
  });

  it('handles cancelled import and export dialogs without mutations', async () => {
    const { commands, files } = service();
    showOpenDialog.mockResolvedValue({ canceled: true, filePaths: [] });
    showSaveDialog.mockResolvedValue({ canceled: true, filePath: undefined });
    await expect(files.importVoiceCommands({} as never)).resolves.toEqual({ status: 'cancelled' });
    await expect(files.exportVoiceCommands({} as never)).resolves.toEqual({ status: 'cancelled' });
    expect(commands.replace).not.toHaveBeenCalled();
  });
});
