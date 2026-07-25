import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import type { BrowserWindow, OpenDialogOptions, SaveDialogOptions } from 'electron';
import { dialog } from 'electron';
import writeFileAtomic from 'write-file-atomic';
import {
  VOCABULARY_FILE_MAX_BYTES,
  type VocabularyFileResult,
} from '../../shared/schemas/vocabulary';
import { PublicAppError } from '../security/public-error';
import type { VocabularyStore } from './vocabulary-store';
import { parseVocabularyText, serializeVocabularyText } from './text-format';

export interface VocabularyDialogPort {
  showOpenDialog(
    owner: BrowserWindow,
    options: OpenDialogOptions,
  ): Promise<
    | { readonly canceled: true; readonly filePaths: readonly string[] }
    | { readonly canceled: false; readonly filePaths: readonly [string, ...string[]] }
  >;
  showSaveDialog(
    owner: BrowserWindow,
    options: SaveDialogOptions,
  ): Promise<
    | { readonly canceled: true; readonly filePath?: undefined }
    | { readonly canceled: false; readonly filePath: string }
  >;
}

const nativeVocabularyDialogs: VocabularyDialogPort = {
  showOpenDialog: async (owner, options) => {
    const result = await dialog.showOpenDialog(owner, options);
    const first = result.filePaths[0];
    return result.canceled || first === undefined
      ? { canceled: true, filePaths: [] }
      : { canceled: false, filePaths: [first, ...result.filePaths.slice(1)] };
  },
  showSaveDialog: async (owner, options) => {
    const result = await dialog.showSaveDialog(owner, options);
    return result.canceled ? { canceled: true } : { canceled: false, filePath: result.filePath };
  },
};

export class VocabularyFileService {
  readonly #store: VocabularyStore;
  readonly #dialogs: VocabularyDialogPort;

  constructor(store: VocabularyStore, dialogs: VocabularyDialogPort = nativeVocabularyDialogs) {
    this.#store = store;
    this.#dialogs = dialogs;
  }

  async importFile(owner: BrowserWindow): Promise<VocabularyFileResult> {
    const selection = await this.#dialogs.showOpenDialog(owner, {
      title: 'Import custom vocabulary',
      properties: ['openFile'],
      filters: [{ name: 'Plain text', extensions: ['txt'] }],
    });
    if (selection.canceled) return { status: 'cancelled' };
    const created = await this.#store.import(
      parseVocabularyText(await readStableFile(selection.filePaths[0])),
    );
    return { status: 'imported', count: created.length };
  }

  async exportFile(owner: BrowserWindow): Promise<VocabularyFileResult> {
    const selection = await this.#dialogs.showSaveDialog(owner, {
      title: 'Export custom vocabulary',
      defaultPath: 'talking-quill-vocabulary.txt',
      filters: [{ name: 'Plain text', extensions: ['txt'] }],
    });
    if (selection.canceled) return { status: 'cancelled' };
    const entries = this.#store.list();
    await writeFileAtomic(
      selection.filePath,
      serializeVocabularyText(entries.map((entry) => entry.value)),
      {
        encoding: 'utf8',
        mode: 0o600,
      },
    );
    return { status: 'exported', count: entries.length };
  }
}

async function readStableFile(path: string): Promise<Uint8Array> {
  let handle: Awaited<ReturnType<typeof open>> | null = null;
  try {
    const pathIdentity = await lstat(path);
    if (!pathIdentity.isFile() || pathIdentity.isSymbolicLink()) invalidFile();
    const noFollow = process.platform === 'win32' ? 0 : constants.O_NOFOLLOW;
    handle = await open(path, constants.O_RDONLY | noFollow);
    const before = await handle.stat();
    if (!before.isFile() || before.dev !== pathIdentity.dev || before.ino !== pathIdentity.ino) {
      invalidFile();
    }
    if (before.size > VOCABULARY_FILE_MAX_BYTES) tooLarge();
    const buffer = Buffer.allocUnsafe(VOCABULARY_FILE_MAX_BYTES + 1);
    let bytesRead = 0;
    while (bytesRead < buffer.byteLength) {
      const chunk = await handle.read(buffer, bytesRead, buffer.byteLength - bytesRead, bytesRead);
      if (chunk.bytesRead === 0) break;
      bytesRead += chunk.bytesRead;
    }
    if (bytesRead > VOCABULARY_FILE_MAX_BYTES) tooLarge();
    const after = await handle.stat();
    if (
      !after.isFile() ||
      after.dev !== before.dev ||
      after.ino !== before.ino ||
      after.size !== before.size ||
      bytesRead !== before.size ||
      after.mtimeMs !== before.mtimeMs ||
      after.ctimeMs !== before.ctimeMs
    ) {
      invalidFile();
    }
    return buffer.subarray(0, bytesRead);
  } catch (error: unknown) {
    if (error instanceof PublicAppError) throw error;
    return invalidFile();
  } finally {
    await handle?.close().catch(() => undefined);
  }
}

function invalidFile(): never {
  throw new PublicAppError({ code: 'BAD_REQUEST', message: 'Select a regular text file.' });
}
function tooLarge(): never {
  throw new PublicAppError({
    code: 'BAD_REQUEST',
    message: 'The vocabulary file is larger than 1 MB.',
  });
}
