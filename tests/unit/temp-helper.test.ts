import { mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

describe('owned repository test directories', () => {
  it('creates a marked contained directory and removes only that literal root', async () => {
    const first = await createTestDirectory('temp-helper');
    const second = await createTestDirectory('temp-helper');
    expect(first).not.toBe(second);
    const marker = JSON.parse(
      await readFile(resolve(first, '.talking-quill-test-owner.json'), 'utf8'),
    ) as { kind: string; root: string; pid: number };
    expect(marker).toMatchObject({
      kind: 'talking-quill-test-directory',
      root: first,
      pid: process.pid,
    });
    await removeTestDirectory(first);
    expect(await readdir(second)).toContain('.talking-quill-test-owner.json');
    await removeTestDirectory(second);
  });

  it('rejects traversal, unregistered roots, and a tampered ownership marker', async () => {
    await expect(createTestDirectory('../outside')).rejects.toThrow('Unsafe test-directory prefix');
    const unregistered = resolve('tmp', 'tests', 'unregistered-owned-fixture');
    await mkdir(unregistered, { recursive: true });
    await expect(removeTestDirectory(unregistered)).rejects.toThrow('unowned test directory');
    await rm(unregistered, { recursive: true, force: true });

    const owned = await createTestDirectory('temp-marker');
    const markerPath = resolve(owned, '.talking-quill-test-owner.json');
    const original = await readFile(markerPath, 'utf8');
    const marker = JSON.parse(original) as { token: string };
    marker.token = '0'.repeat(64);
    await writeFile(markerPath, JSON.stringify(marker), 'utf8');
    await expect(removeTestDirectory(owned)).rejects.toThrow('ownership marker mismatch');
    await writeFile(markerPath, original, 'utf8');
    await removeTestDirectory(owned);
  });
});
