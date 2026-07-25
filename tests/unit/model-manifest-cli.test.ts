import { spawnSync } from 'node:child_process';
import { copyFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

describe('model manifest CLI output isolation', () => {
  it('checks an injected canonical path and rejects an injected stale path', async () => {
    const root = await createTestDirectory('manifest-cli');
    roots.push(root);
    const output = join(root, 'model-manifest.json');
    await copyFile('scripts/model-manifest.json', output);
    expect(runCheck(output).status).toBe(0);
    await writeFile(output, '{"stale":true}\n');
    const stale = runCheck(output);
    expect(stale.status).not.toBe(0);
    expect(stale.stderr).toContain('Invalid live model manifest envelope');
  });
});

function runCheck(output: string) {
  return spawnSync(
    process.execPath,
    ['scripts/model-manifest.mjs', '--check', '--output', output],
    { encoding: 'utf8', windowsHide: true },
  );
}
