import { createHash } from 'node:crypto';
import { mkdir, mkdtemp, readFile, readdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  PERSONAL_TARGETS,
  createFreshEnvironment,
  packagePaths,
  removeWindowsInstallerStaging,
  requireMatchingArtifactSha256,
  withStagedWindowsInstaller,
} from '../../scripts/personal-use.mjs';

const directories: string[] = [];
async function stagingParent() {
  const tmp = resolve('tmp');
  await mkdir(tmp, { recursive: true });
  const directory = await mkdtemp(resolve(tmp, 'personal-tooling-test-'));
  directories.push(directory);
  return directory;
}

afterEach(async () => {
  vi.restoreAllMocks();
  await Promise.all(
    directories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

const artifactBytes = Buffer.from('checked installer bytes');
const checked = {
  artifactBytes,
  sha256: createHash('sha256').update(artifactBytes).digest('hex'),
};

describe('personal Windows installer tooling', () => {
  it.each(['win', 'win-arm64'] as const)('keeps %s builds fresh and unsigned', (target) => {
    const configuration = PERSONAL_TARGETS[target];
    expect(configuration.packageTarget).toBe(`${target}-unsigned`);
    expect(createFreshEnvironment(configuration, {})).toMatchObject({
      TALKING_QUILL_PACKAGE_MODE: 'fresh',
      TALKING_QUILL_PERSONAL_FRESH_INSTALL: '1',
    });
    expect(packagePaths(configuration).installer).toMatch(
      new RegExp(`-win-${configuration.architecture}-setup\\.exe$`),
    );
  });

  it('launches the checked bytes and hash, then removes staging', async () => {
    const parent = await stagingParent();
    await expect(
      withStagedWindowsInstaller(
        checked,
        async (path, hash) => {
          expect(await readFile(path)).toEqual(artifactBytes);
          expect(hash).toBe(checked.sha256);
          return 42;
        },
        parent,
      ),
    ).resolves.toBe(42);
    expect(await readdir(parent)).toEqual([]);
  });

  it('removes staging after a launch failure and retains the original error', async () => {
    const parent = await stagingParent();
    const failure = new Error('launch failed');
    await expect(
      withStagedWindowsInstaller(
        checked,
        () => {
          throw failure;
        },
        parent,
      ),
    ).rejects.toBe(failure);
    expect(await readdir(parent)).toEqual([]);
  });

  it('retains the launch error when cleanup also fails', async () => {
    const parent = await stagingParent();
    const failure = new Error('launch failed');
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    await expect(
      withStagedWindowsInstaller(
        checked,
        () => {
          throw failure;
        },
        parent,
        () => Promise.reject(new Error('cleanup failed')),
      ),
    ).rejects.toBe(failure);
  });

  it('retries transient cleanup failures with bounded backoff', async () => {
    const failure = new Error('file busy');
    const remove = vi.fn().mockRejectedValueOnce(failure).mockResolvedValue(undefined);
    const wait = vi.fn().mockResolvedValue(undefined);
    await removeWindowsInstallerStaging('staging', remove, wait, 3);
    expect(remove).toHaveBeenCalledTimes(2);
    expect(wait).toHaveBeenCalledExactlyOnceWith(100);
  });

  it('reports exhausted cleanup retries with their cause', async () => {
    const failure = new Error('file busy');
    const remove = vi.fn().mockRejectedValue(failure);
    const wait = vi.fn().mockResolvedValue(undefined);
    await expect(removeWindowsInstallerStaging('staging', remove, wait, 2)).rejects.toMatchObject({
      cause: failure,
    });
    expect(remove).toHaveBeenCalledTimes(2);
    expect(wait).toHaveBeenCalledExactlyOnceWith(100);
  });

  it('rejects changed artifacts and malformed expected digests', () => {
    expect(() => requireMatchingArtifactSha256(checked.sha256, checked.sha256)).not.toThrow();
    expect(() => requireMatchingArtifactSha256(checked.sha256, '0'.repeat(64))).toThrow('changed');
    expect(() => requireMatchingArtifactSha256('invalid', 'invalid')).toThrow('changed');
  });
});
