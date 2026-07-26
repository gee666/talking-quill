import { lstat, mkdir, readFile, readdir, symlink, writeFile } from 'node:fs/promises';
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

    await scavengeSessionArtifacts(sessions, 1);

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

  it('does not recursively create a missing immediate parent', async () => {
    const root = await createTestDirectory('session-artifacts-parent-missing');
    roots.push(root);
    const missingParent = join(root, 'tmp');
    const sessions = join(missingParent, 'sessions');

    await expect(scavengeSessionArtifacts(sessions)).rejects.toMatchObject({ code: 'ENOENT' });

    await expect(lstat(missingParent)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('does no filesystem work when cleanup is already aborted', async () => {
    const root = await createTestDirectory('session-artifacts-aborted');
    roots.push(root);
    const sessions = join(root, 'sessions');
    const abort = new AbortController();
    abort.abort();

    await scavengeSessionArtifacts(sessions, 1, abort.signal);

    await expect(lstat(sessions)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('refuses a symlink or junction cleanup root without deleting its target', async (context) => {
    const root = await createTestDirectory('session-artifacts-link');
    const external = await createTestDirectory('session-artifacts-external');
    roots.push(root, external);
    const sentinel = join(external, 'keep.txt');
    await writeFile(sentinel, 'keep');
    const sessions = join(root, 'sessions');
    if (!(await createDirectoryLink(external, sessions))) {
      context.skip('Directory links are unavailable on this platform');
      return;
    }

    await expect(scavengeSessionArtifacts(sessions)).rejects.toThrow(/owned director/i);

    expect(await readFile(sentinel, 'utf8')).toBe('keep');
  });

  it('allows a symlinked higher ancestor when the immediate parent is ordinary', async (context) => {
    const root = await createTestDirectory('session-artifacts-higher-link');
    const external = await createTestDirectory('session-artifacts-higher-external');
    roots.push(root, external);
    await mkdir(join(external, 'tmp'));
    const profile = join(root, 'profile');
    if (!(await createDirectoryLink(external, profile))) {
      context.skip('Directory links are unavailable on this platform');
      return;
    }
    const sessions = join(profile, 'tmp', 'sessions');

    await scavengeSessionArtifacts(sessions);

    expect(await readdir(join(external, 'tmp', 'sessions'))).toEqual([]);
  });

  it('refuses a parent symlink or junction before creating the cleanup leaf', async (context) => {
    const root = await createTestDirectory('session-artifacts-parent-link');
    const external = await createTestDirectory('session-artifacts-parent-external');
    roots.push(root, external);
    const sentinel = join(external, 'keep.txt');
    await writeFile(sentinel, 'keep');
    const temporary = join(root, 'tmp');
    if (!(await createDirectoryLink(external, temporary))) {
      context.skip('Directory links are unavailable on this platform');
      return;
    }
    const sessions = join(temporary, 'sessions');

    await expect(scavengeSessionArtifacts(sessions)).rejects.toThrow(/owned director/i);

    expect(await readFile(sentinel, 'utf8')).toBe('keep');
    await expect(lstat(join(external, 'sessions'))).rejects.toMatchObject({ code: 'ENOENT' });
  });
});

async function createDirectoryLink(target: string, path: string): Promise<boolean> {
  try {
    await symlink(target, path, process.platform === 'win32' ? 'junction' : 'dir');
    return true;
  } catch (error: unknown) {
    if (
      isNodeError(error) &&
      (error.code === 'EPERM' ||
        error.code === 'EACCES' ||
        error.code === 'ENOTSUP' ||
        error.code === 'ENOSYS')
    ) {
      return false;
    }
    throw error;
  }
}

function isNodeError(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && 'code' in error;
}
