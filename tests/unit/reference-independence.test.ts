import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  compareBlobInventory,
  pathContainsReferenceComponent,
} from '../../scripts/reference-independence.mjs';

describe('deleted reference independence', () => {
  const reference = [
    { path: 'reference/nested/source.bin', blob: 'a'.repeat(40) },
    { path: 'reference/other/source.txt', blob: 'b'.repeat(40) },
  ];

  it('ships a complete content-addressed upstream inventory without requiring deleted git objects', () => {
    const inventory = JSON.parse(
      readFileSync('scripts/reference-independence-inventory.json', 'utf8'),
    ) as {
      commit: string;
      inventorySha256: string;
      entries: { blob: string; path: string }[];
    };
    const allowlist = JSON.parse(
      readFileSync('scripts/reference-independence-allowlist.json', 'utf8'),
    ) as { referenceInventoryCommit: string };
    const digest = createHash('sha256')
      .update(inventory.entries.map(({ blob, path }) => `${blob} ${path}\n`).join(''))
      .digest('hex');
    expect(inventory.entries.length).toBeGreaterThan(5_000);
    expect(inventory.commit).toBe(allowlist.referenceInventoryCommit);
    expect(digest).toBe(inventory.inventorySha256);
    expect(readFileSync('scripts/reference-independence.mjs', 'utf8')).not.toContain('cat-file');
  });

  it('rejects a reference component at every depth and spelling', () => {
    expect(pathContainsReferenceComponent('reference/file')).toBe(true);
    expect(pathContainsReferenceComponent('assets/Reference/file')).toBe(true);
    expect(pathContainsReferenceComponent('assets\\reference\\file')).toBe(true);
    expect(pathContainsReferenceComponent('assets/my-reference/file')).toBe(false);
  });

  it('detects renamed binary and nested copies by blob identity', () => {
    expect(
      compareBlobInventory(
        [
          { path: 'app/assets/renamed.jpeg', blob: 'a'.repeat(40) },
          { path: 'deep/nested/copied.ts', blob: 'b'.repeat(40) },
        ],
        reference,
        { entries: [] },
      ),
    ).toEqual([
      `copied-blob:app/assets/renamed.jpeg:${'a'.repeat(40)}`,
      `copied-blob:deep/nested/copied.ts:${'b'.repeat(40)}`,
    ]);
  });

  it('permits only exact current path, blob, and source inventory exceptions', () => {
    const current = [{ path: 'app/assets/logo.png', blob: 'a'.repeat(40) }];
    const exact = {
      entries: [
        {
          path: 'app/assets/logo.png',
          blob: 'a'.repeat(40),
          sourcePaths: ['reference/nested/source.bin'],
          reason: 'Exact attributed binary asset exception.',
        },
      ],
    };
    expect(compareBlobInventory(current, reference, exact)).toEqual([]);
    expect(
      compareBlobInventory(
        [{ path: 'app/assets/copied.png', blob: 'a'.repeat(40) }],
        reference,
        exact,
      ),
    ).toEqual([
      `copied-blob:app/assets/copied.png:${'a'.repeat(40)}`,
      'stale-allowlist:app/assets/logo.png',
    ]);
  });
});
