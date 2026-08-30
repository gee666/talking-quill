import { resolve } from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  FAILURE_CLEANUP_REQUESTS,
  prepareHelperHarnessExecutable,
} from '../native/helper-harness-support.mjs';

describe('native helper harness support', () => {
  it('requests disabled native state before planned shutdown', () => {
    expect(FAILURE_CLEANUP_REQUESTS).toEqual([
      ['session.set_capture', { mode: 'off' }],
      ['activation.configure', { enabled: false, bindings: [] }],
      ['shutdown', {}],
    ]);
  });

  it('does not repackage an explicit non-source helper', async () => {
    const helper = resolve('tmp', 'installed-layout', 'resources', 'helper', 'helper.exe');
    const prepared = await prepareHelperHarnessExecutable({
      helper,
      repositoryRoot: resolve('.'),
      platform: 'win32',
      architecture: 'x64',
    });

    expect(prepared).toMatchObject({ executable: helper, staged: false });
    await expect(prepared.cleanup()).resolves.toBeUndefined();
  });
});
