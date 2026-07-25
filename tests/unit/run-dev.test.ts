import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const repositoryRoot = resolve(import.meta.dirname, '../..');

describe('development command forwarding', () => {
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
