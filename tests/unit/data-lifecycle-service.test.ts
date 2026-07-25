import { lstat, mkdir, readFile, rename, rm, symlink, writeFile } from 'node:fs/promises';
import { dirname, join, parse, resolve } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  DataLifecycleService as BaseDataLifecycleService,
  type DataLifecycleOptions,
  type IdentityBoundRemovalRequest,
  type ResetFaultPhase,
  resetJournalPath,
  resetOwnedApplicationData,
  validateUserDataRoot,
} from '../../app/src/main/data/data-lifecycle-service';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const owned: string[] = [];

async function identityBoundTestRemoval(request: IdentityBoundRemovalRequest): Promise<void> {
  const metadata = await lstat(request.path, { bigint: true });
  const identity = `${String(metadata.dev)}:${String(metadata.ino)}`;
  if (
    metadata.isSymbolicLink() ||
    !metadata.isDirectory() ||
    identity !== request.expectedFileIdentity
  ) {
    throw new Error('test identity-bound boundary rejected a replaced directory');
  }
  await rm(request.path, { recursive: true });
}

class DataLifecycleService extends BaseDataLifecycleService {
  constructor(root: string, options: DataLifecycleOptions) {
    super(root, { removeIdentityBoundDirectory: identityBoundTestRemoval, ...options });
  }
}

afterEach(async () => {
  await Promise.all(owned.splice(0).map((path) => removeTestDirectory(path)));
});

async function fixture(prefix: string) {
  const parent = await createTestDirectory(prefix);
  owned.push(parent);
  const allowedBase = join(parent, 'app-data');
  const root = join(allowedBase, 'Talking Quill');
  const homeDirectory = join(parent, 'home');
  await Promise.all([mkdir(root, { recursive: true }), mkdir(homeDirectory, { recursive: true })]);
  const options = {
    allowedBase,
    homeDirectory,
    removeIdentityBoundDirectory: identityBoundTestRemoval,
  } as const;
  const service = new DataLifecycleService(root, options);
  await service.initializeOwnership();
  return { parent, allowedBase, root, homeDirectory, options, service };
}

describe('DataLifecycleService', () => {
  it('journals a full reset outside the data root and recovers it on startup', async () => {
    const { parent, root, options, service } = await fixture('data-reset');
    const ollama = join(parent, 'ollama-models');
    await mkdir(join(root, 'models'), { recursive: true });
    await mkdir(ollama, { recursive: true });
    await Promise.all([
      writeFile(join(root, 'settings.json'), 'private settings'),
      writeFile(join(root, 'history.db'), 'private history'),
      writeFile(join(root, 'models', 'weight.onnx'), 'model'),
      writeFile(join(ollama, 'manifest'), 'must survive'),
    ]);

    await service.prepareReset();
    expect(service.resetPrepared).toBe(true);
    expect(dirname(service.journalPath)).toBe(resolve(dirname(root)));
    expect(service.journalPath.startsWith(`${resolve(root)}${parse(root).root}`)).toBe(false);

    const restarted = new DataLifecycleService(root, options);
    await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: true });
    await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(lstat(service.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(readFile(join(ollama, 'manifest'), 'utf8')).resolves.toBe('must survive');
  });

  it.each([
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
  ] satisfies readonly ResetFaultPhase[])(
    'recovers idempotently after a %s crash',
    async (phase) => {
      const { parent, root, allowedBase, homeDirectory } = await fixture(`data-fault-${phase}`);
      const outside = join(parent, 'external-sentinel');
      await Promise.all([
        writeFile(join(root, 'private-data'), 'delete me'),
        writeFile(outside, 'preserve me'),
      ]);
      let injected = false;
      const crashing = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        removeIdentityBoundDirectory: identityBoundTestRemoval,
        injectResetFault: (candidate) => {
          if (!injected && candidate === phase) {
            injected = true;
            return Promise.reject(new Error(`crash:${phase}`));
          }
          return Promise.resolve();
        },
      });
      if (phase === 'after-journal-write') {
        await expect(crashing.prepareReset()).rejects.toThrow(`crash:${phase}`);
      } else {
        await crashing.prepareReset();
        await expect(crashing.recoverPendingReset()).rejects.toThrow(`crash:${phase}`);
      }

      const restarted = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        removeIdentityBoundDirectory: identityBoundTestRemoval,
      });
      if (phase === 'after-journal-remove') {
        await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: false });
      } else {
        await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: true });
      }
      await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' });
      await expect(lstat(restarted.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
      await expect(readFile(outside, 'utf8')).resolves.toBe('preserve me');
      await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: false });
    },
  );

  it('finishes a renamed partial tombstone without requiring its ownership marker', async () => {
    const { root, allowedBase, homeDirectory } = await fixture('data-partial-tombstone');
    await writeFile(join(root, 'remaining-data'), 'private');
    const crashing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: (phase) =>
        phase === 'after-live-rename'
          ? Promise.reject(new Error('crash after rename'))
          : Promise.resolve(),
    });
    await crashing.prepareReset();
    await expect(crashing.recoverPendingReset()).rejects.toThrow('crash after rename');
    const journal = JSON.parse(await readFile(crashing.journalPath, 'utf8')) as {
      tombstonePath: string;
    };
    await Promise.all([
      rm(join(journal.tombstonePath, '.talking-quill-owner.json'), { force: true }),
      rm(join(journal.tombstonePath, 'remaining-data'), { force: true }),
    ]);

    await expect(
      resetOwnedApplicationData(root, {
        allowedBase,
        homeDirectory,
        removeIdentityBoundDirectory: identityBoundTestRemoval,
      }),
    ).resolves.toBe(true);
    await expect(lstat(journal.tombstonePath)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(lstat(crashing.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('rejects concurrent live and tombstone roots without deleting either', async () => {
    const { parent, root, allowedBase, homeDirectory } = await fixture('data-reset-ambiguity');
    const crashing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: (phase) =>
        phase === 'after-live-rename'
          ? Promise.reject(new Error('crash after rename'))
          : Promise.resolve(),
    });
    await crashing.prepareReset();
    await expect(crashing.recoverPendingReset()).rejects.toThrow('crash after rename');
    const journal = JSON.parse(await readFile(crashing.journalPath, 'utf8')) as {
      tombstonePath: string;
    };
    await mkdir(root);
    await Promise.all([
      writeFile(join(root, 'new-live-sentinel'), 'live'),
      writeFile(join(parent, 'outside-sentinel'), 'outside'),
    ]);

    await expect(resetOwnedApplicationData(root, { allowedBase, homeDirectory })).rejects.toThrow(
      'ambiguous',
    );
    await expect(readFile(join(root, 'new-live-sentinel'), 'utf8')).resolves.toBe('live');
    await expect(lstat(journal.tombstonePath)).resolves.toBeDefined();
    await expect(readFile(join(parent, 'outside-sentinel'), 'utf8')).resolves.toBe('outside');
  });

  it('restores and preserves a replacement moved by the before-rename race hook', async () => {
    const { parent, root, allowedBase, homeDirectory } = await fixture(
      'data-before-rename-replacement',
    );
    const original = join(parent, 'original-owned-root');
    const marker = await readFile(join(root, '.talking-quill-owner.json'));
    const racing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: async (phase) => {
        if (phase !== 'before-live-rename') return;
        await rename(root, original);
        await mkdir(root);
        await Promise.all([
          writeFile(join(root, '.talking-quill-owner.json'), marker),
          writeFile(join(root, 'attacker-replacement'), 'preserve replacement'),
        ]);
      },
    });
    await racing.prepareReset();
    await expect(racing.recoverPendingReset()).rejects.toThrow('remains quarantined');
    const journal = JSON.parse(await readFile(racing.journalPath, 'utf8')) as {
      tombstonePath: string;
    };
    const preservedPath = process.platform === 'win32' ? root : journal.tombstonePath;
    await expect(readFile(join(preservedPath, 'attacker-replacement'), 'utf8')).resolves.toBe(
      'preserve replacement',
    );
    await expect(lstat(original)).resolves.toBeDefined();
    await expect(lstat(racing.journalPath)).resolves.toBeDefined();

    const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(restarted.recoverPendingReset()).rejects.toThrow(
      process.platform === 'win32'
        ? 'ownership could not be verified'
        : 'journal-recorded directory',
    );
    await expect(readFile(join(preservedPath, 'attacker-replacement'), 'utf8')).resolves.toBe(
      'preserve replacement',
    );
  });

  it.each(['directory', 'symlink'] as const)(
    'retains an unverified tombstone when the live restore destination becomes occupied by a %s',
    async (occupancy) => {
      const { parent, root, allowedBase, homeDirectory } = await fixture(
        `data-rename-occupied-${occupancy}`,
      );
      const original = join(parent, 'original-owned-root');
      const outside = join(parent, 'outside-live-target');
      const marker = await readFile(join(root, '.talking-quill-owner.json'));
      let tombstonePath = '';
      const racing = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        injectResetFault: async (phase) => {
          if (phase === 'before-live-rename') {
            await rename(root, original);
            await mkdir(root);
            await Promise.all([
              writeFile(join(root, '.talking-quill-owner.json'), marker),
              writeFile(join(root, 'moved-replacement'), 'quarantine me'),
            ]);
          }
          if (phase === 'before-renamed-identity-check') {
            const journal = JSON.parse(await readFile(racing.journalPath, 'utf8')) as {
              tombstonePath: string;
            };
            tombstonePath = journal.tombstonePath;
            if (occupancy === 'directory') {
              await mkdir(root);
              await writeFile(join(root, 'occupied-live'), 'preserve live');
            } else {
              await mkdir(outside);
              await writeFile(join(outside, 'outside-sentinel'), 'preserve outside');
              await symlink(outside, root, process.platform === 'win32' ? 'junction' : 'dir');
            }
          }
        },
      });
      await racing.prepareReset();
      await expect(racing.recoverPendingReset()).rejects.toThrow('remains quarantined');
      await expect(readFile(join(tombstonePath, 'moved-replacement'), 'utf8')).resolves.toBe(
        'quarantine me',
      );
      await expect(lstat(racing.journalPath)).resolves.toBeDefined();
      if (occupancy === 'directory') {
        await expect(readFile(join(root, 'occupied-live'), 'utf8')).resolves.toBe('preserve live');
      } else {
        await expect(readFile(join(outside, 'outside-sentinel'), 'utf8')).resolves.toBe(
          'preserve outside',
        );
      }
      const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
      await expect(restarted.recoverPendingReset()).rejects.toThrow('ambiguous');
      await expect(readFile(join(tombstonePath, 'moved-replacement'), 'utf8')).resolves.toBe(
        'quarantine me',
      );
    },
  );

  it('rejects replacement of the prepared live root and preserves it', async () => {
    const { parent, root, allowedBase, homeDirectory, service } =
      await fixture('data-root-replaced');
    await service.prepareReset();
    const original = join(parent, 'original-owned-root');
    await rename(root, original);
    await mkdir(root);
    await writeFile(join(root, 'replacement-sentinel'), 'preserve replacement');
    await writeFile(
      join(root, '.talking-quill-owner.json'),
      await readFile(join(original, '.talking-quill-owner.json')),
    );

    const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(restarted.recoverPendingReset()).rejects.toThrow(
      'ownership could not be verified',
    );
    await expect(readFile(join(root, 'replacement-sentinel'), 'utf8')).resolves.toBe(
      'preserve replacement',
    );
  });

  it('rejects a tombstone replaced by a link and preserves its external target', async () => {
    const { parent, root, allowedBase, homeDirectory } = await fixture('data-tombstone-link');
    const crashing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: (phase) =>
        phase === 'after-live-rename'
          ? Promise.reject(new Error('crash after rename'))
          : Promise.resolve(),
    });
    await crashing.prepareReset();
    await expect(crashing.recoverPendingReset()).rejects.toThrow('crash after rename');
    const journal = JSON.parse(await readFile(crashing.journalPath, 'utf8')) as {
      tombstonePath: string;
    };
    const outside = join(parent, 'outside-tombstone-target');
    await rm(journal.tombstonePath, { recursive: true });
    await mkdir(outside);
    await writeFile(join(outside, 'sentinel'), 'preserve');
    await symlink(
      outside,
      journal.tombstonePath,
      process.platform === 'win32' ? 'junction' : 'dir',
    );

    const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(restarted.recoverPendingReset()).rejects.toThrow('journal-recorded directory');
    await expect(readFile(join(outside, 'sentinel'), 'utf8')).resolves.toBe('preserve');
  });

  it('preserves a regular-directory replacement of a recorded tombstone', async () => {
    const { parent, root, allowedBase, homeDirectory } = await fixture(
      'data-tombstone-directory-replacement',
    );
    const crashing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: (phase) =>
        phase === 'after-live-rename'
          ? Promise.reject(new Error('crash after rename'))
          : Promise.resolve(),
    });
    await crashing.prepareReset();
    await expect(crashing.recoverPendingReset()).rejects.toThrow('crash after rename');
    const journal = JSON.parse(await readFile(crashing.journalPath, 'utf8')) as {
      tombstonePath: string;
    };
    await rm(journal.tombstonePath, { recursive: true });
    await mkdir(journal.tombstonePath);
    await Promise.all([
      writeFile(join(journal.tombstonePath, 'replacement-sentinel'), 'preserve'),
      writeFile(join(parent, 'external-sentinel'), 'external'),
    ]);

    await expect(resetOwnedApplicationData(root, { allowedBase, homeDirectory })).rejects.toThrow(
      'journal-recorded directory',
    );
    await expect(
      readFile(join(journal.tombstonePath, 'replacement-sentinel'), 'utf8'),
    ).resolves.toBe('preserve');
    await expect(readFile(join(parent, 'external-sentinel'), 'utf8')).resolves.toBe('external');
    await expect(lstat(crashing.journalPath)).resolves.toBeDefined();
  });

  it.each(['directory', 'symlink'] as const)(
    'preserves a %s replacement at the final disposal-transition race hook',
    async (replacementKind) => {
      const { parent, root, allowedBase, homeDirectory } = await fixture(
        `data-final-transition-${replacementKind}`,
      );
      const outside = join(parent, 'final-transition-outside');
      let tombstonePath = '';
      let disposalPath = '';
      const racing = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        injectResetFault: async (phase) => {
          if (phase !== 'before-tombstone-disposal-transition') return;
          const journal = JSON.parse(await readFile(racing.journalPath, 'utf8')) as {
            tombstonePath: string;
            disposalPath: string;
          };
          tombstonePath = journal.tombstonePath;
          disposalPath = journal.disposalPath;
          await rm(tombstonePath, { recursive: true });
          if (replacementKind === 'directory') {
            await mkdir(tombstonePath);
            await writeFile(join(tombstonePath, 'replacement-sentinel'), 'preserve replacement');
          } else {
            await mkdir(outside);
            await writeFile(join(outside, 'outside-sentinel'), 'preserve outside');
            await symlink(
              outside,
              tombstonePath,
              process.platform === 'win32' ? 'junction' : 'dir',
            );
          }
        },
      });
      await racing.prepareReset();
      await expect(racing.recoverPendingReset()).rejects.toThrow('remains quarantined');
      await expect(lstat(racing.journalPath)).resolves.toBeDefined();
      if (replacementKind === 'directory') {
        const preservedPath = process.platform === 'win32' ? tombstonePath : disposalPath;
        await expect(readFile(join(preservedPath, 'replacement-sentinel'), 'utf8')).resolves.toBe(
          'preserve replacement',
        );
      } else {
        await expect(readFile(join(outside, 'outside-sentinel'), 'utf8')).resolves.toBe(
          'preserve outside',
        );
        await expect(
          lstat(disposalPath).then((metadata) => metadata.isSymbolicLink()),
        ).resolves.toBe(true);
      }
      const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
      await expect(restarted.recoverPendingReset()).rejects.toThrow(/journal-recorded|ambiguous/u);
    },
  );

  it.each(['live', 'tombstone'] as const)(
    'fails closed when a %s root appears beside a crash-retained disposal directory',
    async (candidate) => {
      const { root, allowedBase, homeDirectory } = await fixture(
        `data-disposal-ambiguity-${candidate}`,
      );
      const crashing = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        injectResetFault: (phase) =>
          phase === 'after-tombstone-disposal-transition'
            ? Promise.reject(new Error('crash after disposal transition'))
            : Promise.resolve(),
      });
      await crashing.prepareReset();
      await expect(crashing.recoverPendingReset()).rejects.toThrow(
        'crash after disposal transition',
      );
      const journal = JSON.parse(await readFile(crashing.journalPath, 'utf8')) as {
        tombstonePath: string;
        disposalPath: string;
      };
      const competingPath = candidate === 'live' ? root : journal.tombstonePath;
      await mkdir(competingPath);
      await writeFile(join(competingPath, 'competing-sentinel'), 'preserve competing');

      const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
      await expect(restarted.recoverPendingReset()).rejects.toThrow('ambiguous');
      await expect(readFile(join(competingPath, 'competing-sentinel'), 'utf8')).resolves.toBe(
        'preserve competing',
      );
      await expect(lstat(journal.disposalPath)).resolves.toBeDefined();
      await expect(lstat(crashing.journalPath)).resolves.toBeDefined();
    },
  );

  it.runIf(process.platform === 'win32')(
    'retains disposal and journal when tombstone restore becomes occupied on Windows',
    async () => {
      const { root, allowedBase, homeDirectory } = await fixture('data-disposal-restore-occupied');
      let tombstonePath = '';
      let disposalPath = '';
      const racing = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        injectResetFault: async (phase) => {
          if (phase === 'before-tombstone-disposal-transition') {
            const journal = JSON.parse(await readFile(racing.journalPath, 'utf8')) as {
              tombstonePath: string;
              disposalPath: string;
            };
            tombstonePath = journal.tombstonePath;
            disposalPath = journal.disposalPath;
            await rm(tombstonePath, { recursive: true });
            await mkdir(tombstonePath);
            await writeFile(join(tombstonePath, 'moved-replacement'), 'preserve moved');
          }
          if (phase === 'after-tombstone-disposal-transition') {
            await mkdir(tombstonePath);
            await writeFile(join(tombstonePath, 'occupied-sentinel'), 'preserve occupied');
          }
        },
      });
      await racing.prepareReset();
      await expect(racing.recoverPendingReset()).rejects.toThrow('remains quarantined');
      await expect(readFile(join(disposalPath, 'moved-replacement'), 'utf8')).resolves.toBe(
        'preserve moved',
      );
      await expect(readFile(join(tombstonePath, 'occupied-sentinel'), 'utf8')).resolves.toBe(
        'preserve occupied',
      );
      await expect(lstat(racing.journalPath)).resolves.toBeDefined();
      const restarted = new DataLifecycleService(root, { allowedBase, homeDirectory });
      await expect(restarted.recoverPendingReset()).rejects.toThrow('ambiguous');
    },
  );

  it('refuses to prepare a new destructive reset without an identity-bound boundary', async () => {
    const { root, allowedBase, homeDirectory, service } = await fixture(
      'data-identity-bound-prepare',
    );
    const unavailable = new BaseDataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(unavailable.prepareReset()).rejects.toThrow(
      'Identity-bound recursive reset deletion is unavailable',
    );
    await expect(lstat(root)).resolves.toBeDefined();
    await expect(lstat(service.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('fails closed with the journal and disposal quarantine intact when no identity-bound boundary exists', async () => {
    const { root, allowedBase, homeDirectory, service } = await fixture(
      'data-identity-bound-unavailable',
    );
    await writeFile(join(root, 'private'), 'preserve until trusted deletion');
    await service.prepareReset();
    const restarted = new BaseDataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(restarted.recoverPendingReset()).rejects.toThrow(
      'Identity-bound recursive reset deletion is unavailable',
    );
    const journal = JSON.parse(await readFile(service.journalPath, 'utf8')) as {
      disposalPath: string;
    };
    await expect(readFile(join(journal.disposalPath, 'private'), 'utf8')).resolves.toBe(
      'preserve until trusted deletion',
    );
    await expect(lstat(service.journalPath)).resolves.toBeDefined();
  });

  it('rejects a deterministic final pathname swap before destructive removal', async () => {
    const { parent, root, allowedBase, homeDirectory } = await fixture('data-final-path-swap');
    let originalDisposal = '';
    let replacementDisposal = '';
    const racing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      removeIdentityBoundDirectory: identityBoundTestRemoval,
      injectResetFault: async (phase) => {
        if (phase !== 'before-identity-bound-remove') return;
        const journal = JSON.parse(await readFile(racing.journalPath, 'utf8')) as {
          disposalPath: string;
        };
        replacementDisposal = journal.disposalPath;
        originalDisposal = join(parent, 'identity-bound-original');
        await rename(replacementDisposal, originalDisposal);
        await mkdir(replacementDisposal);
        await writeFile(join(replacementDisposal, 'replacement'), 'must survive');
      },
    });
    await racing.prepareReset();
    await expect(racing.recoverPendingReset()).rejects.toThrow(
      'identity-bound boundary rejected a replaced directory',
    );
    await expect(lstat(originalDisposal)).resolves.toBeDefined();
    await expect(readFile(join(replacementDisposal, 'replacement'), 'utf8')).resolves.toBe(
      'must survive',
    );
    await expect(lstat(racing.journalPath)).resolves.toBeDefined();
  });

  it('is idempotent when no reset is pending', async () => {
    const { root, service } = await fixture('data-no-reset');
    await expect(service.recoverPendingReset()).resolves.toEqual({ recovered: false });
    await expect(lstat(root)).resolves.toBeDefined();
  });

  it.each([
    { name: 'renamed-root publication', failAt: 1, removedBeforeFailure: false },
    { name: 'identity-bound removal publication', failAt: 3, removedBeforeFailure: true },
  ])(
    'retains recovery authority across a $name durability fault',
    async ({ failAt, removedBeforeFailure }) => {
      const { root, allowedBase, homeDirectory } = await fixture(
        `data-durability-${String(failAt)}`,
      );
      await writeFile(join(root, 'private'), 'recover me');
      let syncCalls = 0;
      const faulting = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        removeIdentityBoundDirectory: identityBoundTestRemoval,
        syncResetDirectory: () => {
          syncCalls += 1;
          return syncCalls === failAt
            ? Promise.reject(new Error('directory durability fault'))
            : Promise.resolve();
        },
      });
      await faulting.prepareReset();
      await expect(faulting.recoverPendingReset()).rejects.toThrow('directory durability fault');
      await expect(lstat(faulting.journalPath)).resolves.toBeDefined();
      if (removedBeforeFailure) await expect(lstat(root)).rejects.toMatchObject({ code: 'ENOENT' });

      const restarted = new DataLifecycleService(root, {
        allowedBase,
        homeDirectory,
        removeIdentityBoundDirectory: identityBoundTestRemoval,
      });
      await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: true });
      await expect(lstat(faulting.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
    },
  );

  it('leaves no prepared reset when atomic journal creation fails', async () => {
    const { root, allowedBase, homeDirectory } = await fixture('data-journal-failure');
    const service = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      writeResetJournal: async () => Promise.reject(new Error('journal write failed')),
    });
    await expect(service.prepareReset()).rejects.toThrow('journal write failed');
    expect(service.resetPrepared).toBe(false);
    await expect(service.cancelPreparedReset()).resolves.toBeUndefined();
    await expect(lstat(service.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(lstat(root)).resolves.toBeDefined();
  });

  it('cancels a valid legacy v2 journal without deleting the live root', async () => {
    const { root, service } = await fixture('data-legacy-journal');
    await writeFile(join(root, 'sentinel'), 'preserve');
    await service.prepareReset();
    const current = JSON.parse(await readFile(service.journalPath, 'utf8')) as Record<
      string,
      unknown
    >;
    delete current.rootFileIdentity;
    delete current.tombstonePath;
    delete current.disposalPath;
    delete current.phase;
    current.schemaVersion = 2;
    await writeFile(service.journalPath, `${JSON.stringify(current)}\n`);

    const restarted = new DataLifecycleService(root, {
      allowedBase: dirname(root),
      homeDirectory: join(dirname(dirname(root)), 'home'),
    });
    await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: false });
    await expect(readFile(join(root, 'sentinel'), 'utf8')).resolves.toBe('preserve');
    await expect(lstat(service.journalPath)).rejects.toMatchObject({ code: 'ENOENT' });
  });

  it('migrates a valid v3 journal forward before any disposal transition', async () => {
    const { root, allowedBase, homeDirectory, service } = await fixture('data-v3-journal');
    await service.prepareReset();
    const current = JSON.parse(await readFile(service.journalPath, 'utf8')) as Record<
      string,
      unknown
    >;
    delete current.disposalPath;
    delete current.phase;
    current.schemaVersion = 3;
    await writeFile(service.journalPath, `${JSON.stringify(current)}\n`);
    const crashing = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      injectResetFault: (phase) =>
        phase === 'before-tombstone-disposal-transition'
          ? Promise.reject(new Error('inspect migrated journal'))
          : Promise.resolve(),
    });
    await expect(crashing.recoverPendingReset()).rejects.toThrow('inspect migrated journal');
    const migrated = JSON.parse(await readFile(service.journalPath, 'utf8')) as {
      schemaVersion: number;
      disposalPath: string;
      phase: string;
    };
    expect(migrated.schemaVersion).toBe(4);
    expect(migrated.disposalPath).toMatch(/\.talking-quill-reset-disposal-/u);
    expect(migrated.phase).toBe('disposal-pending');

    const restarted = new DataLifecycleService(root, {
      allowedBase,
      homeDirectory,
      removeIdentityBoundDirectory: identityBoundTestRemoval,
    });
    await expect(restarted.recoverPendingReset()).resolves.toEqual({ recovered: true });
  });

  it('requires a canonical ownership marker before preparing deletion', async () => {
    const parent = await createTestDirectory('data-unowned');
    owned.push(parent);
    const allowedBase = join(parent, 'app-data');
    const root = join(allowedBase, 'Talking Quill');
    const homeDirectory = join(parent, 'home');
    await Promise.all([mkdir(root, { recursive: true }), mkdir(homeDirectory)]);
    const service = new DataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(service.prepareReset()).rejects.toThrow();
    await expect(lstat(root)).resolves.toBeDefined();
  });

  it('rejects a mismatched journal without deleting owned data', async () => {
    const { root, service } = await fixture('data-hostile-reset');
    const other = join(dirname(root), 'other-profile');
    await writeFile(join(root, 'keep'), 'safe');
    await service.prepareReset();
    const journal = JSON.parse(await readFile(service.journalPath, 'utf8')) as Record<
      string,
      unknown
    >;
    journal.userDataRoot = other;
    await writeFile(service.journalPath, `${JSON.stringify(journal)}\n`);
    await expect(service.recoverPendingReset()).rejects.toThrow('does not match');
    await expect(readFile(join(root, 'keep'), 'utf8')).resolves.toBe('safe');
  });

  it('rejects a root symlink or Windows junction that escapes the allowed base', async () => {
    const parent = await createTestDirectory('data-root-link');
    owned.push(parent);
    const allowedBase = join(parent, 'app-data');
    const outside = join(parent, 'outside');
    const root = join(allowedBase, 'Talking Quill');
    const homeDirectory = join(parent, 'home');
    await Promise.all([
      mkdir(allowedBase, { recursive: true }),
      mkdir(outside),
      mkdir(homeDirectory),
    ]);
    await symlink(outside, root, process.platform === 'win32' ? 'junction' : 'dir');
    const service = new DataLifecycleService(root, { allowedBase, homeDirectory });
    await expect(service.initializeOwnership()).rejects.toThrow(/symbolic-link|junction/i);
  });

  it('removes a nested link without traversing into its external target', async () => {
    const { parent, root, options, service } = await fixture('data-nested-link');
    const outside = join(parent, 'outside');
    await mkdir(outside);
    await writeFile(join(outside, 'ollama-model'), 'must survive');
    await symlink(
      outside,
      join(root, 'linked-provider'),
      process.platform === 'win32' ? 'junction' : 'dir',
    );
    await service.prepareReset();
    await service.recoverPendingReset();
    await expect(readFile(join(outside, 'ollama-model'), 'utf8')).resolves.toBe('must survive');

    await mkdir(root, { recursive: true });
    const replacement = new DataLifecycleService(root, options);
    await replacement.initializeOwnership();
    await expect(resetOwnedApplicationData(root, options)).resolves.toBe(true);
  });

  it('refuses filesystem roots, home directories, profile ancestors, and roots outside the allowed base', async () => {
    const parent = await createTestDirectory('data-unsafe-roots');
    owned.push(parent);
    const homeDirectory = join(parent, 'profile', 'user');
    const allowedBase = join(homeDirectory, 'AppData', 'Roaming');
    await mkdir(allowedBase, { recursive: true });
    expect(() => validateUserDataRoot(parse(resolve('.')).root)).toThrow('unsafe');
    expect(
      () =>
        new DataLifecycleService(homeDirectory, {
          allowedBase: parent,
          homeDirectory,
        }),
    ).toThrow(/home|profile/i);
    expect(
      () =>
        new DataLifecycleService(join(parent, 'profile'), {
          allowedBase: parent,
          homeDirectory,
        }),
    ).toThrow(/home|profile/i);
    expect(
      () =>
        new DataLifecycleService(join(parent, 'elsewhere'), {
          allowedBase,
          homeDirectory,
        }),
    ).toThrow(/allowed base/i);
    expect(resetJournalPath(join(allowedBase, 'Talking Quill'))).toBe(
      resetJournalPath(resolve(allowedBase, 'Talking Quill')),
    );
  });
});
