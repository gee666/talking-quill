import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const repositoryRoot = resolve(import.meta.dirname, '../..');

describe('development command forwarding', () => {
  it('always stages the feature-free structural gateway', () => {
    const runDev = readFileSync(resolve(repositoryRoot, 'scripts/run-dev.mjs'), 'utf8');
    const buildHelper = readFileSync(resolve(repositoryRoot, 'scripts/build-helper.mjs'), 'utf8');

    expect(runDev).toContain("runNode('scripts/build-helper.mjs')");
    expect(runDev).not.toContain('TALKING_QUILL_TRANSACTIONAL_SHORTCUTS_DEV');
    expect(buildHelper).not.toContain('transactional-shortcuts-dev');
    expect(buildHelper).toContain("'-p'");
    expect(buildHelper).toContain("'talking-quill-helper'");
    expect(buildHelper).toContain("'--no-default-features'");
  });

  it.skipIf(process.env.NODE_V8_COVERAGE !== undefined)(
    'strips one pnpm separator and prints help without launching Electron',
    () => {
      const windows = process.platform === 'win32';
      const result = spawnSync(
        windows ? (process.env.ComSpec ?? 'C:\\Windows\\System32\\cmd.exe') : 'pnpm',
        windows ? ['/d', '/s', '/c', 'pnpm.cmd dev -- --help'] : ['dev', '--', '--help'],
        {
          cwd: repositoryRoot,
          encoding: 'utf8',
          timeout: 30_000,
          windowsHide: true,
        },
      );

      expect(result.error).toBeUndefined();
      expect(result.status).toBe(0);
      expect(`${result.stdout}${result.stderr}`).toContain('electron-vite');
      expect(`${result.stdout}${result.stderr}`).not.toContain(
        'Built isolated, standalone preload',
      );
    },
    35_000,
  );
});
