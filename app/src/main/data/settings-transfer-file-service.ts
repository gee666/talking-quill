import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import type { BrowserWindow, OpenDialogOptions, SaveDialogOptions } from 'electron';
import { dialog } from 'electron';
import writeFileAtomic from 'write-file-atomic';
import {
  DictationProfilesTransferSchema,
  SETTINGS_TRANSFER_FILE_MAX_BYTES,
  VoiceCommandsTransferSchema,
  type SettingsTransferResult,
} from '../../shared/schemas/settings-transfer';
import type { VoiceCommandStore } from '../commands/voice-command-store';
import type { EchoSessionController } from '../echo/echo-session-controller';
import { PublicAppError } from '../security/public-error';

export interface SettingsTransferDialogPort {
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

const nativeDialogs: SettingsTransferDialogPort = {
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

export class SettingsTransferFileService {
  readonly #commands: VoiceCommandStore;
  readonly #echo: EchoSessionController;
  readonly #dialogs: SettingsTransferDialogPort;

  constructor(
    commands: VoiceCommandStore,
    echo: EchoSessionController,
    dialogs: SettingsTransferDialogPort = nativeDialogs,
  ) {
    this.#commands = commands;
    this.#echo = echo;
    this.#dialogs = dialogs;
  }

  async importVoiceCommands(owner: BrowserWindow): Promise<SettingsTransferResult> {
    const path = await this.#selectImport(owner, 'Import voice commands');
    if (path === null) return { status: 'cancelled' };
    const transfer = VoiceCommandsTransferSchema.parse(await readTransferJson(path));
    await this.#commands.replace(transfer.commands);
    return { status: 'imported', count: transfer.commands.length };
  }

  async exportVoiceCommands(owner: BrowserWindow): Promise<SettingsTransferResult> {
    const path = await this.#selectExport(
      owner,
      'Export voice commands',
      'talking-quill-voice-commands.json',
    );
    if (path === null) return { status: 'cancelled' };
    const commands = this.#commands.list();
    await writeTransferJson(path, {
      format: 'talking-quill.voice-commands',
      version: 1,
      commands,
    });
    return { status: 'exported', count: commands.length };
  }

  async importDictationProfiles(owner: BrowserWindow): Promise<SettingsTransferResult> {
    const path = await this.#selectImport(owner, 'Import dictation profiles');
    if (path === null) return { status: 'cancelled' };
    const transfer = DictationProfilesTransferSchema.parse(await readTransferJson(path));
    await this.#echo.replaceProfiles(transfer.profiles);
    return { status: 'imported', count: transfer.profiles.length };
  }

  async exportDictationProfiles(owner: BrowserWindow): Promise<SettingsTransferResult> {
    const path = await this.#selectExport(
      owner,
      'Export dictation profiles',
      'talking-quill-dictation-profiles.json',
    );
    if (path === null) return { status: 'cancelled' };
    const profiles = this.#echo.dictationProfiles;
    await writeTransferJson(path, {
      format: 'talking-quill.dictation-profiles',
      version: 1,
      profiles,
    });
    return { status: 'exported', count: profiles.length };
  }

  async #selectImport(owner: BrowserWindow, title: string): Promise<string | null> {
    const selection = await this.#dialogs.showOpenDialog(owner, {
      title,
      properties: ['openFile'],
      filters: [{ name: 'Talking Quill JSON', extensions: ['json'] }],
    });
    return selection.canceled ? null : selection.filePaths[0];
  }

  async #selectExport(
    owner: BrowserWindow,
    title: string,
    defaultPath: string,
  ): Promise<string | null> {
    const selection = await this.#dialogs.showSaveDialog(owner, {
      title,
      defaultPath,
      filters: [{ name: 'Talking Quill JSON', extensions: ['json'] }],
    });
    return selection.canceled ? null : selection.filePath;
  }
}

async function readTransferJson(path: string): Promise<unknown> {
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
    if (before.size > SETTINGS_TRANSFER_FILE_MAX_BYTES) tooLarge();
    const buffer = Buffer.allocUnsafe(SETTINGS_TRANSFER_FILE_MAX_BYTES + 1);
    let bytesRead = 0;
    while (bytesRead < buffer.byteLength) {
      const chunk = await handle.read(buffer, bytesRead, buffer.byteLength - bytesRead, bytesRead);
      if (chunk.bytesRead === 0) break;
      bytesRead += chunk.bytesRead;
    }
    if (bytesRead > SETTINGS_TRANSFER_FILE_MAX_BYTES) tooLarge();
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
    try {
      return JSON.parse(buffer.subarray(0, bytesRead).toString('utf8')) as unknown;
    } catch {
      return invalidFile();
    }
  } catch (error: unknown) {
    if (error instanceof PublicAppError) throw error;
    return invalidFile();
  } finally {
    await handle?.close().catch(() => undefined);
  }
}

async function writeTransferJson(path: string, value: unknown): Promise<void> {
  await writeFileAtomic(path, `${JSON.stringify(value, null, 2)}\n`, {
    encoding: 'utf8',
    mode: 0o600,
  });
}

function invalidFile(): never {
  throw new PublicAppError({
    code: 'BAD_REQUEST',
    message: 'Select a valid Talking Quill JSON file.',
  });
}

function tooLarge(): never {
  throw new PublicAppError({
    code: 'BAD_REQUEST',
    message: 'The import file is larger than 1 MB.',
  });
}
