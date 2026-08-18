import { readFile, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const showOpenDialog = vi.fn();
const showSaveDialog = vi.fn();
vi.mock('electron', () => ({ dialog: { showOpenDialog, showSaveDialog } }));

const { SettingsTransferFileService } =
  await import('../../app/src/main/data/settings-transfer-file-service');

const directories: string[] = [];
afterEach(async () => {
  vi.clearAllMocks();
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

function service() {
  const commands = {
    list: vi.fn(() => DEFAULT_SETTINGS.voiceCommands),
    replace: vi.fn(() => Promise.resolve(DEFAULT_SETTINGS.voiceCommands)),
  };
  const echo = {
    dictationProfiles: DEFAULT_SETTINGS.dictationProfiles,
    replaceProfiles: vi.fn(() => Promise.resolve(DEFAULT_SETTINGS)),
  };
  return {
    commands,
    echo,
    files: new SettingsTransferFileService(commands as never, echo as never),
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

  it('round-trips all dictation profiles through the activation coordinator boundary', async () => {
    const directory = await createTestDirectory('profile-transfer');
    directories.push(directory);
    const path = join(directory, 'profiles.json');
    const { echo, files } = service();
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: path });
    await expect(files.exportDictationProfiles({} as never)).resolves.toEqual({
      status: 'exported',
      count: DEFAULT_SETTINGS.dictationProfiles.length,
    });

    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    await expect(files.importDictationProfiles({} as never)).resolves.toEqual({
      status: 'imported',
      count: DEFAULT_SETTINGS.dictationProfiles.length,
    });
    expect(echo.replaceProfiles).toHaveBeenCalledWith(DEFAULT_SETTINGS.dictationProfiles);
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
