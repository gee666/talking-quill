import { lstat, mkdir, rm } from 'node:fs/promises';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import {
  DataLifecycleService,
  type ResetFaultPhase,
} from '../../app/src/main/data/data-lifecycle-service';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

it('preserves recovery fault order and publishes completion only after journal removal', async () => {
  const parent = await createTestDirectory('reset-recovery-order');
  try {
    const root = join(parent, 'data');
    const homeDirectory = join(parent, 'home');
    await Promise.all([mkdir(root), mkdir(homeDirectory)]);
    const phases: ResetFaultPhase[] = [];
    const service = new DataLifecycleService(root, {
      allowedBase: parent,
      homeDirectory,
      injectResetFault: async (phase) => {
        phases.push(phase);
        expect(service.resetPrepared).toBe(phase !== 'after-journal-remove');
        if (phase === 'before-journal-remove') {
          await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' });
          await expect(lstat(service.journalPath)).resolves.toBeDefined();
        }
        if (phase === 'after-journal-remove') {
          await expect(lstat(service.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
        }
      },
      removeIdentityBoundDirectory: async ({ path, expectedFileIdentity }) => {
        const metadata = await lstat(path, { bigint: true });
        expect(`${String(metadata.dev)}:${String(metadata.ino)}`).toBe(expectedFileIdentity);
        await rm(path, { recursive: true });
      },
    });
    await service.initializeOwnership();
    await service.prepareReset();
    await expect(service.recoverPendingReset()).resolves.toEqual({ recovered: true });
    expect(phases).toEqual([
      'after-journal-write',
      'before-live-rename',
      'before-renamed-identity-check',
      'after-live-rename',
      'before-tombstone-remove',
      'before-tombstone-disposal-transition',
      'after-tombstone-disposal-transition',
      'before-disposal-remove',
      'before-identity-bound-remove',
      'after-tombstone-remove',
      'before-journal-remove',
      'after-journal-remove',
    ]);
    await expect(service.recoverPendingReset()).resolves.toEqual({ recovered: false });
    expect(phases).toHaveLength(12);
  } finally {
    await removeTestDirectory(parent);
  }
});
