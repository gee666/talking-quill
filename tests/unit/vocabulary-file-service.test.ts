import { appendFile, readFile, symlink, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { VOCABULARY_FILE_MAX_BYTES } from '../../app/src/shared/schemas/vocabulary';
import { SETTINGS_MIGRATIONS } from '../../app/src/main/persistence/settings-migrations';
import { VocabularyStore } from '../../app/src/main/vocabulary/vocabulary-store';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const showOpenDialog = vi.fn();
const showSaveDialog = vi.fn();
vi.mock('electron', () => ({ dialog: { showOpenDialog, showSaveDialog } }));

const { VocabularyFileService } = await import('../../app/src/main/vocabulary/file-service');

const directories: string[] = [];
afterEach(async () => {
  vi.clearAllMocks();
  await Promise.all(directories.splice(0).map(removeTestDirectory));
});

function service(values: string[] = []) {
  const store = {
    list: () =>
      values.map((value, index) => ({ id: String(index), value, createdAt: 1, updatedAt: 1 })),
    import: vi.fn((imported: readonly string[]) =>
      Promise.resolve(imported.map((value) => ({ value }))),
    ),
  };
  return { store, files: new VocabularyFileService(store as never) };
}

describe('VocabularyFileService', () => {
  it('imports a user-selected regular UTF-8 file without accepting renderer paths', async () => {
    const directory = await createTestDirectory('vocabulary-file');
    directories.push(directory);
    const path = join(directory, 'words.txt');
    await writeFile(path, 'GraphQL\nJosé\n', 'utf8');
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    const { store, files } = service();
    await expect(files.importFile({} as never)).resolves.toEqual({ status: 'imported', count: 2 });
    expect(store.import).toHaveBeenCalledWith(['GraphQL', 'José']);
  });

  it('rejects symbolic links and leaves the store untouched', async () => {
    const directory = await createTestDirectory('vocabulary-link');
    directories.push(directory);
    const target = join(directory, 'target.txt');
    const link = join(directory, 'link.txt');
    await writeFile(target, 'GraphQL\n', 'utf8');
    await symlink(target, link);
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [link] });
    const { store, files } = service();
    await expect(files.importFile({} as never)).rejects.toThrow('regular text file');
    expect(store.import).not.toHaveBeenCalled();
  });

  it('rejects a bounded max-plus-one read when a selected file grows concurrently', async () => {
    const directory = await createTestDirectory('vocabulary-growing-file');
    directories.push(directory);
    const path = join(directory, 'growing.txt');
    await writeFile(path, Buffer.alloc(VOCABULARY_FILE_MAX_BYTES, 0x61));
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [path] });
    const { store, files } = service();
    const growth = appendFile(path, 'b');
    await expect(files.importFile({} as never)).rejects.toThrow(
      /larger than 1 MB|regular text file/,
    );
    await growth;
    expect(store.import).not.toHaveBeenCalled();
  });

  it('round-trips persisted vocabulary through export into a fresh store', async () => {
    const directory = await createTestDirectory('vocabulary-roundtrip');
    directories.push(directory);
    const exportPath = join(directory, 'roundtrip.txt');
    const sourceSettings = new SettingsStore(join(directory, 'source.json'), {
      migrations: SETTINGS_MIGRATIONS,
    });
    await sourceSettings.initialize();
    const sourceStore = new VocabularyStore(sourceSettings);
    await sourceStore.import(['GraphQL', 'José', 'AnythingLLM']);
    showSaveDialog.mockResolvedValue({ canceled: false, filePath: exportPath });
    await expect(new VocabularyFileService(sourceStore).exportFile({} as never)).resolves.toEqual({
      status: 'exported',
      count: 3,
    });

    const freshSettings = new SettingsStore(join(directory, 'fresh.json'), {
      migrations: SETTINGS_MIGRATIONS,
    });
    await freshSettings.initialize();
    const freshStore = new VocabularyStore(freshSettings);
    showOpenDialog.mockResolvedValue({ canceled: false, filePaths: [exportPath] });
    await expect(new VocabularyFileService(freshStore).importFile({} as never)).resolves.toEqual({
      status: 'imported',
      count: 3,
    });
    await freshSettings.flush();
    const reopened = new SettingsStore(join(directory, 'fresh.json'), {
      migrations: SETTINGS_MIGRATIONS,
    });
    await reopened.initialize();
    expect(reopened.get().customVocabulary.map((entry) => entry.value)).toEqual([
      'GraphQL',
      'José',
      'AnythingLLM',
    ]);
  });

  it('exports deterministic LF text atomically and handles cancelled dialogs', async () => {
    const directory = await createTestDirectory('vocabulary-export');
    directories.push(directory);
    const path = join(directory, 'export.txt');
    showSaveDialog.mockResolvedValueOnce({ canceled: true, filePath: undefined });
    const { files } = service(['GraphQL', 'José']);
    await expect(files.exportFile({} as never)).resolves.toEqual({ status: 'cancelled' });
    showSaveDialog.mockResolvedValueOnce({ canceled: false, filePath: path });
    await expect(files.exportFile({} as never)).resolves.toEqual({ status: 'exported', count: 2 });
    expect(await readFile(path, 'utf8')).toBe('GraphQL\nJosé\n');
  });
});
