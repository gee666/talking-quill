import { constants } from 'node:fs';
import { lstat, open, realpath, rm } from 'node:fs/promises';
import { basename, dirname, isAbsolute, relative, resolve } from 'node:path';
import { syncDirectory } from '../persistence/atomic-json';

export const UNINSTALL_RESET_ENV = 'TALKING_QUILL_UNINSTALL_RESET_CHALLENGE';

/**
 * Accidental-invocation guard for the uninstaller helper. This is not an authorization boundary:
 * another process running as the same user can inspect the environment and temporary files.
 */
export async function consumeUninstallResetChallenge(
  challengePath: string,
  temporaryDirectory: string,
  environment: NodeJS.ProcessEnv = process.env,
  platform: NodeJS.Platform = process.platform,
): Promise<void> {
  const token = environment[UNINSTALL_RESET_ENV];
  if (
    token === undefined ||
    token.length < 8 ||
    token.length > 32_768 ||
    challengePath.length > 32_768
  ) {
    throw new Error('Uninstall reset challenge is unavailable.');
  }
  const absolutePath = resolve(challengePath);
  const challengeBasename = basename(absolutePath);
  const expectedBasename = 'talking-quill-reset.challenge';
  const basenameMatches =
    platform === 'win32'
      ? challengeBasename.toLowerCase() === expectedBasename
      : challengeBasename === expectedBasename;
  if (
    !isAbsolute(challengePath) ||
    challengeBasename.normalize('NFC') !== challengeBasename ||
    !basenameMatches
  ) {
    throw new Error('Uninstall reset challenge path is invalid.');
  }
  const [canonicalTemporary, canonicalParent] = await Promise.all([
    realpath(temporaryDirectory),
    realpath(dirname(absolutePath)),
  ]);
  const suffix = relative(canonicalTemporary, canonicalParent);
  if (
    suffix.length === 0 ||
    suffix === '..' ||
    suffix.startsWith('../') ||
    suffix.startsWith('..\\')
  ) {
    throw new Error('Uninstall reset challenge is outside the temporary directory.');
  }
  const before = await lstat(absolutePath);
  if (!before.isFile() || before.isSymbolicLink() || before.size > 32_768) {
    throw new Error('Uninstall reset challenge is not a bounded regular file.');
  }
  const handle = await open(absolutePath, constants.O_RDONLY | constants.O_NOFOLLOW);
  let source: string;
  try {
    const opened = await handle.stat();
    if (
      !opened.isFile() ||
      opened.size !== before.size ||
      opened.dev !== before.dev ||
      opened.ino !== before.ino
    ) {
      throw new Error('Uninstall reset challenge changed during validation.');
    }
    source = await handle.readFile('utf8');
    const after = await lstat(absolutePath);
    if (
      after.isSymbolicLink() ||
      after.size !== opened.size ||
      after.dev !== opened.dev ||
      after.ino !== opened.ino
    ) {
      throw new Error('Uninstall reset challenge changed during validation.');
    }
  } finally {
    await handle.close();
  }
  await rm(absolutePath, { force: true });
  await syncDirectory(dirname(absolutePath));
  if (source !== token) throw new Error('Uninstall reset challenge did not match.');
}
