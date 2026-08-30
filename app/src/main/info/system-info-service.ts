import { app, dialog, shell, type BrowserWindow } from 'electron';
import { createHash, randomBytes } from 'node:crypto';
import { open, readFile, rename, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { z } from 'zod';
import { HelperReadinessSchema } from '../../shared/schemas/helper-readiness';
import type { InfoLocation, InfoPermission } from '../../shared/schemas/info';
import { DiagnosticLogEntrySchema, OwnerAggregateFileSchema } from '../security/diagnostic-logger';
import { redactSensitive } from '../security/redaction';
import type { AppPaths } from '../persistence/paths';
import { PublicAppError } from '../security/public-error';
import { validateReleaseUrl } from './release-url-policy';

const DiagnosticSummarySchema = z
  .object({
    appVersion: z.string().regex(/^\d+\.\d+\.\d+$/u),
    platform: z.enum(['win32', 'darwin', 'linux']),
    architecture: z.enum(['x64', 'arm64']),
    helper: HelperReadinessSchema,
    nativeLaunchFailure: z
      .string()
      .regex(/^[a-z0-9][a-z0-9_-]{0,95}$/u)
      .nullable(),
    settings: z.union([
      z
        .object({
          code: z.literal('INVALID_SETTINGS_RECOVERED'),
          reason: z.enum(['parse', 'schema', 'migration']),
          preservedAt: z.string().nullable(),
        })
        .strict()
        .transform(({ code, reason, preservedAt }) => ({
          code,
          reason,
          preserved: preservedAt !== null,
        })),
      z
        .object({
          code: z.literal('UNSUPPORTED_SETTINGS_VERSION'),
          foundVersion: z.number().int().nonnegative(),
        })
        .strict(),
      z
        .object({
          code: z.literal('SETTINGS_IO_ERROR'),
          operation: z.enum(['read', 'write', 'preserve-invalid']),
        })
        .strict(),
      z.null(),
    ]),
  })
  .strict();

const MAC_PERMISSION_URLS: Readonly<Record<Exclude<InfoPermission, 'microphone'>, string>> = {
  accessibility: 'x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility',
  'input-monitoring': 'x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent',
  'screen-recording':
    'x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture',
};

export class SystemInfoService {
  readonly #paths: AppPaths;
  readonly #openMicrophoneSettings: () => Promise<void>;

  constructor(paths: AppPaths, openMicrophoneSettings: () => Promise<void>) {
    this.#paths = paths;
    this.#openMicrophoneSettings = openMicrophoneSettings;
  }

  async openPermission(permission: InfoPermission): Promise<void> {
    if (permission === 'microphone') {
      await mapOpenFailure(this.#openMicrophoneSettings);
      return;
    }
    if (process.platform !== 'darwin') {
      throw new PublicAppError({
        code: 'UNAVAILABLE',
        message: 'This permission pane is unavailable.',
      });
    }
    await mapOpenFailure(() => shell.openExternal(MAC_PERMISSION_URLS[permission]));
  }

  async openLocation(location: InfoLocation): Promise<void> {
    await mapOpenFailure(async () => {
      const error = await shell.openPath(location === 'data' ? this.#paths.root : this.#paths.logs);
      if (error.length > 0) throw new Error('Electron could not open the folder');
    });
  }

  async openRelease(value: string): Promise<void> {
    const releaseUrl = validateReleaseUrl(value);
    await mapOpenFailure(() => shell.openExternal(releaseUrl));
  }

  async exportDiagnostics(
    owner: BrowserWindow,
    summary: Readonly<Record<string, unknown>>,
  ): Promise<'cancelled' | 'exported'> {
    const selection = await dialog.showSaveDialog(owner, {
      title: 'Export Talking Quill diagnostic logs',
      defaultPath: join(app.getPath('downloads'), 'talking-quill-diagnostics.zip'),
      filters: [{ name: 'ZIP archive', extensions: ['zip'] }],
      properties: ['createDirectory', 'showOverwriteConfirmation'],
    });
    if (selection.canceled) return 'cancelled';

    const generatedAt = Date.now();
    const files: { readonly name: string; readonly contents: Buffer }[] = [];
    const metadata = redactSensitive({
      format: 'talking-quill.diagnostics',
      version: 1,
      generatedAt,
      summary: DiagnosticSummarySchema.parse(summary),
      privacy:
        'Allowlisted operational and redacted debug events only. No typed text, key events, audio, transcripts, models, voice commands, tokens, secrets, usernames or SIDs.',
    });
    files.push({
      name: 'metadata.json',
      contents: Buffer.from(`${JSON.stringify(metadata, null, 2)}\n`),
    });
    for (const suffix of ['.3', '.2', '.1', '']) {
      const source = await readFile(
        join(this.#paths.logs, `diagnostic.jsonl${suffix}`),
        'utf8',
      ).catch((error: unknown) => {
        if (isNodeError(error) && error.code === 'ENOENT') return '';
        throw error;
      });
      const accepted: string[] = [];
      for (const line of source.split('\n')) {
        if (line.length === 0) continue;
        try {
          const parsed = DiagnosticLogEntrySchema.safeParse(JSON.parse(line) as unknown);
          if (parsed.success) accepted.push(JSON.stringify(redactSensitive(parsed.data)));
        } catch {
          // Ignore a partial crash tail or a record from an incompatible version.
        }
      }
      if (accepted.length > 0) {
        files.push({
          name: `logs/diagnostic.jsonl${suffix}`,
          contents: Buffer.from(`${accepted.join('\n')}\n`),
        });
      }
    }
    const ownerAggregateSource = await readFile(
      join(this.#paths.logs, 'owner-connection-counts.json'),
      'utf8',
    ).catch((error: unknown) => {
      if (isNodeError(error) && error.code === 'ENOENT') return null;
      throw error;
    });
    if (ownerAggregateSource !== null) {
      const aggregate = OwnerAggregateFileSchema.safeParse(JSON.parse(ownerAggregateSource));
      if (aggregate.success) {
        files.push({
          name: 'logs/owner-connection-counts.json',
          contents: Buffer.from(`${JSON.stringify(aggregate.data, null, 2)}\n`),
        });
      }
    }
    const manifest = {
      format: 'talking-quill.diagnostic-integrity',
      version: 1,
      generatedAt,
      files: files.map((file) => ({
        path: file.name,
        bytes: file.contents.length,
        sha256: createHash('sha256').update(file.contents).digest('hex'),
      })),
    };
    files.push({
      name: 'manifest.json',
      contents: Buffer.from(`${JSON.stringify(manifest, null, 2)}\n`),
    });
    const temporary = `${selection.filePath}.tmp-${String(process.pid)}-${randomBytes(8).toString('hex')}`;
    let handle: Awaited<ReturnType<typeof open>> | null = null;
    try {
      handle = await open(temporary, 'wx', 0o600);
      await handle.writeFile(createStoredZip(files));
      await handle.sync();
      await handle.close();
      handle = null;
      await rename(temporary, selection.filePath);
    } finally {
      await handle?.close().catch(() => undefined);
      await rm(temporary, { force: true }).catch(() => undefined);
    }
    return 'exported';
  }
}

function createStoredZip(
  files: readonly { readonly name: string; readonly contents: Buffer }[],
): Buffer {
  const local: Buffer[] = [];
  const central: Buffer[] = [];
  let offset = 0;
  const { date, time } = zipTimestamp(new Date());
  for (const file of files) {
    if (!/^[a-z0-9./-]+$/u.test(file.name)) throw new Error('Invalid diagnostic archive path');
    const name = Buffer.from(file.name, 'utf8');
    const checksum = crc32(file.contents);
    const localHeader = Buffer.alloc(30);
    localHeader.writeUInt32LE(0x04034b50, 0);
    localHeader.writeUInt16LE(20, 4);
    localHeader.writeUInt16LE(0x0800, 6);
    localHeader.writeUInt16LE(time, 10);
    localHeader.writeUInt16LE(date, 12);
    localHeader.writeUInt32LE(checksum, 14);
    localHeader.writeUInt32LE(file.contents.length, 18);
    localHeader.writeUInt32LE(file.contents.length, 22);
    localHeader.writeUInt16LE(name.length, 26);
    local.push(localHeader, name, file.contents);

    const centralHeader = Buffer.alloc(46);
    centralHeader.writeUInt32LE(0x02014b50, 0);
    centralHeader.writeUInt16LE(20, 4);
    centralHeader.writeUInt16LE(20, 6);
    centralHeader.writeUInt16LE(0x0800, 8);
    centralHeader.writeUInt16LE(time, 12);
    centralHeader.writeUInt16LE(date, 14);
    centralHeader.writeUInt32LE(checksum, 16);
    centralHeader.writeUInt32LE(file.contents.length, 20);
    centralHeader.writeUInt32LE(file.contents.length, 24);
    centralHeader.writeUInt16LE(name.length, 28);
    centralHeader.writeUInt32LE(offset, 42);
    central.push(centralHeader, name);
    offset += localHeader.length + name.length + file.contents.length;
  }
  const centralSize = central.reduce((total, value) => total + value.length, 0);
  const end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50, 0);
  end.writeUInt16LE(files.length, 8);
  end.writeUInt16LE(files.length, 10);
  end.writeUInt32LE(centralSize, 12);
  end.writeUInt32LE(offset, 16);
  return Buffer.concat([...local, ...central, end]);
}

function zipTimestamp(now: Date): { readonly date: number; readonly time: number } {
  const year = Math.max(1980, Math.min(2107, now.getFullYear()));
  return {
    date: ((year - 1980) << 9) | ((now.getMonth() + 1) << 5) | now.getDate(),
    time: (now.getHours() << 11) | (now.getMinutes() << 5) | Math.floor(now.getSeconds() / 2),
  };
}

function crc32(contents: Buffer): number {
  let crc = 0xffffffff;
  for (const byte of contents) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit += 1) {
      crc = (crc >>> 1) ^ (crc & 1 ? 0xedb88320 : 0);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}

async function mapOpenFailure(operation: () => Promise<unknown>): Promise<void> {
  try {
    await operation();
  } catch {
    throw new PublicAppError({
      code: 'UNAVAILABLE',
      message: 'The requested system location could not be opened.',
    });
  }
}
