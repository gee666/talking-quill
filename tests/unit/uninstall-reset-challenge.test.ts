import { lstat, mkdir, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import {
  consumeUninstallResetChallenge,
  UNINSTALL_RESET_ENV,
} from '../../app/src/main/data/uninstall-reset-challenge';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

let temporary = '';
beforeEach(async () => {
  temporary = await createTestDirectory('uninstall-challenge');
});
afterEach(async () => removeTestDirectory(temporary));

describe('uninstall reset accidental-invocation challenge', () => {
  it('consumes a matching one-time file below the OS temporary root', async () => {
    const plugin = resolve(temporary, 'nsis-random-plugin-dir');
    const path = resolve(plugin, 'talking-quill-reset.challenge');
    await mkdir(plugin);
    await writeFile(path, plugin);
    await expect(
      consumeUninstallResetChallenge(path, temporary, { [UNINSTALL_RESET_ENV]: plugin }),
    ).resolves.toBeUndefined();
    await expect(lstat(path)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(
      consumeUninstallResetChallenge(path, temporary, { [UNINSTALL_RESET_ENV]: plugin }),
    ).rejects.toThrow();
  });

  it('uses an exact basename with Windows case semantics and rejects suffix matches', async () => {
    const plugin = resolve(temporary, 'unicode-e\u0301-plugin');
    await mkdir(plugin);
    const uppercase = resolve(plugin, 'TALKING-QUILL-RESET.CHALLENGE');
    await writeFile(uppercase, 'windows-token');
    await expect(
      consumeUninstallResetChallenge(
        uppercase,
        temporary,
        { [UNINSTALL_RESET_ENV]: 'windows-token' },
        'win32',
      ),
    ).resolves.toBeUndefined();

    for (const basename of [
      'prefix-talking-quill-reset.challenge',
      'talking-quill-reset.challenge.bak',
      'talking-quill-reset.challenge\u0301',
    ]) {
      const path = resolve(plugin, basename);
      await writeFile(path, 'token-value');
      await expect(
        consumeUninstallResetChallenge(
          path,
          temporary,
          { [UNINSTALL_RESET_ENV]: 'token-value' },
          'win32',
        ),
      ).rejects.toThrow('path is invalid');
    }

    const posixCase = resolve(plugin, 'Talking-Quill-Reset.challenge');
    await writeFile(posixCase, 'posix-token');
    await expect(
      consumeUninstallResetChallenge(
        posixCase,
        temporary,
        { [UNINSTALL_RESET_ENV]: 'posix-token' },
        'darwin',
      ),
    ).rejects.toThrow('path is invalid');
  });

  it('rejects missing, mismatched, and outside challenges', async () => {
    const plugin = resolve(temporary, 'plugin');
    const path = resolve(plugin, 'talking-quill-reset.challenge');
    await mkdir(plugin);
    await writeFile(path, 'expected-token');
    await expect(consumeUninstallResetChallenge(path, temporary, {})).rejects.toThrow();
    await expect(
      consumeUninstallResetChallenge(path, temporary, {
        [UNINSTALL_RESET_ENV]: 'different-token',
      }),
    ).rejects.toThrow('did not match');

    const outsideRoot = await createTestDirectory('uninstall-outside');
    try {
      const outsidePlugin = resolve(outsideRoot, 'plugin');
      const outside = resolve(outsidePlugin, 'talking-quill-reset.challenge');
      await mkdir(outsidePlugin);
      await writeFile(outside, outsidePlugin);
      await expect(
        consumeUninstallResetChallenge(outside, temporary, {
          [UNINSTALL_RESET_ENV]: outsidePlugin,
        }),
      ).rejects.toThrow('outside');
    } finally {
      await removeTestDirectory(outsideRoot);
    }
  });
});
