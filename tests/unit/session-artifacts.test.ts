import { mkdir, readdir, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { scavengeSessionArtifacts } from '../../app/src/main/echo/session-artifacts';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));

describe('session artifact scavenging', () => {
  it('removes only entries below the dedicated session directory', async () => {
    const root = await createTestDirectory('session-artifacts');
    roots.push(root);
    const sessions = join(root, 'sessions');
    await mkdir(join(sessions, 'stale-directory'), { recursive: true });
    await writeFile(join(sessions, 'stale-file.pcm'), 'stale');
    await writeFile(join(sessions, 'stale-directory', 'nested'), 'stale');
    const sibling = join(root, 'keep.txt');
    await writeFile(sibling, 'keep');

    await scavengeSessionArtifacts(sessions);

    expect(await readdir(sessions)).toEqual([]);
    expect(await readdir(root)).toContain('keep.txt');
  });

  it('creates a missing owned directory', async () => {
    const root = await createTestDirectory('session-artifacts-missing');
    roots.push(root);
    const sessions = join(root, 'sessions');
    await scavengeSessionArtifacts(sessions);
    expect(await readdir(sessions)).toEqual([]);
  });
});
