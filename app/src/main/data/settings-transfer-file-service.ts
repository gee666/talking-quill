import { constants } from 'node:fs';
import { lstat, open } from 'node:fs/promises';
import type {
  BrowserWindow,
  MessageBoxOptions,
  OpenDialogOptions,
  SaveDialogOptions,
} from 'electron';
import { dialog } from 'electron';
import writeFileAtomic from 'write-file-atomic';
import {
  DictationProfilesTransferSchema,
  DictationProfilesTransferV1Schema,
  DictationProfilesTransferV2Schema,
  SETTINGS_TRANSFER_FILE_MAX_BYTES,
  VoiceCommandsTransferSchema,
  type SettingsTransferResult,
} from '../../shared/schemas/settings-transfer';
import { DictationProfileListSchema } from '../../shared/schemas/dictation-profiles';
import { shortcutPlatformPolicy } from '../../shared/schemas/shortcut-platform-policy';
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
  showMessageBox(
    owner: BrowserWindow,
    options: MessageBoxOptions,
  ): Promise<{ readonly response: number }>;
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
  showMessageBox: (owner, options) => dialog.showMessageBox(owner, options),
};

export class SettingsTransferFileService {
  readonly #commands: VoiceCommandStore;
  readonly #echo: EchoSessionController;
  readonly #dialogs: SettingsTransferDialogPort;
  readonly #platform: string;

  constructor(
    commands: VoiceCommandStore,
    echo: EchoSessionController,
    dialogs: SettingsTransferDialogPort = nativeDialogs,
    platform: string = process.platform,
  ) {
    this.#commands = commands;
    this.#echo = echo;
    this.#dialogs = dialogs;
    this.#platform = platform;
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
    const profiles = DictationProfileListSchema.parse(transfer.profiles);
    const policies = profiles.map((profile) =>
      shortcutPlatformPolicy(profile.shortcut, this.#platform),
    );
    const impossible = policies.find((policy) => policy.status === 'impossible');
    if (impossible?.status === 'impossible') {
      throw new PublicAppError({ code: 'BAD_REQUEST', message: impossible.message });
    }
    const riskyCount = policies.filter((policy) => policy.status === 'risky').length;
    if (riskyCount > 0) {
      const confirmation = await this.#dialogs.showMessageBox(owner, {
        type: 'warning',
        title: 'Import risky global shortcuts?',
        message: `${String(riskyCount)} imported shortcut${riskyCount === 1 ? '' : 's'} may overlap operating-system or application input.`,
        detail:
          'Review these shortcuts after import. While global capture is enabled, risky shortcuts can intercept input in other applications.',
        buttons: ['Import profiles', 'Cancel'],
        defaultId: 1,
        cancelId: 1,
        noLink: true,
      });
      if (confirmation.response !== 0) return { status: 'cancelled' };
    }
    await this.#echo.replaceProfiles(profiles);
    return { status: 'imported', count: profiles.length };
  }

  async exportDictationProfiles(owner: BrowserWindow): Promise<SettingsTransferResult> {
    const path = await this.#selectExport(
      owner,
      'Export dictation profiles',
      'talking-quill-dictation-profiles.json',
    );
    if (path === null) return { status: 'cancelled' };
    const profiles = this.#echo.dictationProfiles;
    const v1Transfer = {
      format: 'talking-quill.dictation-profiles' as const,
      version: 1 as const,
      profiles,
    };
    const transfer = DictationProfilesTransferV1Schema.safeParse(v1Transfer);
    await writeTransferJson(
      path,
      transfer.success
        ? transfer.data
        : DictationProfilesTransferV2Schema.parse({ ...v1Transfer, version: 2 }),
    );
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
