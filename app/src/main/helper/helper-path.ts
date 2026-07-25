import { constants } from 'node:fs';
import { access, lstat } from 'node:fs/promises';
import { isAbsolute, join, resolve } from 'node:path';

export type HelperPlatform = 'win32' | 'darwin';

export interface HelperPathContext {
  readonly packaged: boolean;
  readonly resourcesPath: string;
  readonly appPath: string;
  readonly platform: NodeJS.Platform;
}

export class HelperBinaryError extends Error {
  readonly reason: 'binary-missing' | 'binary-invalid';

  constructor(reason: 'binary-missing' | 'binary-invalid') {
    super(
      reason === 'binary-missing' ? 'Native helper binary is missing' : 'Native helper is invalid',
    );
    this.name = 'HelperBinaryError';
    this.reason = reason;
  }
}

export function helperExecutableName(platform: HelperPlatform): string {
  return platform === 'win32' ? 'talking-quill-helper.exe' : 'talking-quill-helper';
}

export function resolveHelperExecutable(context: HelperPathContext): string {
  if (context.platform !== 'win32' && context.platform !== 'darwin') {
    throw new HelperBinaryError('binary-invalid');
  }
  const name = helperExecutableName(context.platform);
  const candidate = context.packaged
    ? join(context.resourcesPath, 'helper', name)
    : join(context.appPath, 'native', name);
  const absolute = resolve(candidate);
  if (!isAbsolute(absolute)) throw new HelperBinaryError('binary-invalid');
  return absolute;
}

export async function validateHelperExecutable(
  executablePath: string,
  platform: HelperPlatform,
): Promise<void> {
  let metadata;
  try {
    metadata = await lstat(executablePath);
  } catch (error) {
    if (isMissingFileError(error)) throw new HelperBinaryError('binary-missing');
    throw new HelperBinaryError('binary-invalid');
  }
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size === 0) {
    throw new HelperBinaryError('binary-invalid');
  }
  try {
    await access(
      executablePath,
      platform === 'darwin' ? constants.R_OK | constants.X_OK : constants.R_OK,
    );
  } catch {
    throw new HelperBinaryError('binary-invalid');
  }
}

function isMissingFileError(error: unknown): boolean {
  return (
    typeof error === 'object' &&
    error !== null &&
    'code' in error &&
    (error.code === 'ENOENT' || error.code === 'ENOTDIR')
  );
}
