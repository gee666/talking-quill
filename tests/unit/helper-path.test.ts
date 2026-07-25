import { chmod, symlink, writeFile } from 'node:fs/promises';
import { join, resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  HelperBinaryError,
  resolveHelperExecutable,
  validateHelperExecutable,
} from '../../app/src/main/helper/helper-path';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const created: string[] = [];
afterEach(async () => Promise.all(created.splice(0).map(removeTestDirectory)));

describe('native helper path policy', () => {
  it('resolves only the known development and packaged locations', () => {
    const appPath = resolve('tmp/tests/fake-app');
    const resourcesPath = resolve('tmp/tests/fake-resources');
    expect(
      resolveHelperExecutable({
        packaged: false,
        resourcesPath,
        appPath,
        platform: 'win32',
      }),
    ).toBe(join(appPath, 'native', 'talking-quill-helper.exe'));
    expect(
      resolveHelperExecutable({
        packaged: true,
        resourcesPath,
        appPath,
        platform: 'darwin',
      }),
    ).toBe(join(resourcesPath, 'helper', 'talking-quill-helper'));
    expect(() =>
      resolveHelperExecutable({
        packaged: true,
        resourcesPath: '/tmp',
        appPath: '/tmp',
        platform: 'linux',
      }),
    ).toThrow(HelperBinaryError);
  });

  it('accepts only non-empty regular executable files and rejects symlinks', async () => {
    const directory = await createTestDirectory('helper-path');
    created.push(directory);
    const binary = join(directory, 'talking-quill-helper');
    await writeFile(binary, 'fixture', { mode: 0o700 });
    await chmod(binary, 0o700);
    await expect(validateHelperExecutable(binary, 'darwin')).resolves.toBeUndefined();

    const link = join(directory, 'linked-helper');
    await symlink(binary, link, 'file');
    await expect(validateHelperExecutable(link, 'darwin')).rejects.toMatchObject({
      reason: 'binary-invalid',
    });
    await expect(
      validateHelperExecutable(join(directory, 'missing'), 'win32'),
    ).rejects.toMatchObject({ reason: 'binary-missing' });
  });
});
