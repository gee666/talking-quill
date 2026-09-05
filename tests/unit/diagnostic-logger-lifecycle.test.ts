import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  DiagnosticLogger,
  OwnerAggregateFileSchema,
} from '../../app/src/main/security/diagnostic-logger';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const owned: string[] = [];
afterEach(async () => {
  await Promise.all(owned.splice(0).map(removeTestDirectory));
});

function replay(count: number) {
  return {
    event: 'helper.owner.connection.replay',
    journalId: '01'.repeat(32),
    journalNonce: '23'.repeat(32),
    streamId: '45'.repeat(32),
    processGeneration: '1',
    category: 'disconnected',
    operation: 'lease.renew',
    correlationStatus: 'pending',
    healthRefresh: 'not_attempted',
    transportStatus: 'eof',
    ownerProcessState: 'running',
    count: String(count),
    counterOverflow: false,
    durable: true,
    durabilityFailures: '0',
    writerStartFailures: '0',
    synchronizationRecoveries: '0',
  };
}

function deferred() {
  let resolve: () => void = () => undefined;
  const promise = new Promise<void>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

async function setup() {
  const root = await createTestDirectory('diagnostic-queue-lifecycle');
  owned.push(root);
  const settings = new SettingsStore(join(root, 'settings.json'));
  await settings.initialize();
  await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
  const started = deferred();
  const release = deferred();
  const checkpoints: Buffer[] = [];
  const write = vi.fn(async (_path: string, contents: Buffer) => {
    checkpoints.push(contents);
    started.resolve();
    await release.promise;
  });
  const logger = new DiagnosticLogger(settings, join(root, 'logs'), { writeOwnerAggregate: write });
  await logger.initialize();
  return {
    logger,
    settings,
    started: started.promise,
    release: release.resolve,
    checkpoints,
    write,
  };
}

describe('diagnostic replay queue lifecycle', () => {
  it('waits for persistence before acknowledging and rechecks privacy for queued replays', async () => {
    const value = await setup();
    try {
      let acknowledged = false;
      const first = value.logger.recordOwnerConnectionReplay(replay(1)).then((result) => {
        acknowledged = true;
        return result;
      });
      await value.started;
      const second = value.logger.recordOwnerConnectionReplay(replay(2));
      await value.settings.update({ privacy: { diagnosticLoggingEnabled: false } });
      expect(acknowledged).toBe(false);
      value.release();
      await expect(first).resolves.toBe(true);
      await expect(second).resolves.toBe(false);
      expect(value.write).toHaveBeenCalledOnce();
      const checkpoint = OwnerAggregateFileSchema.parse(
        JSON.parse(value.checkpoints[0]?.toString() ?? 'null'),
      );
      expect(checkpoint.acceptedDisconnects).toBe('1');
    } finally {
      value.release();
      await value.logger.dispose();
    }
  });

  it('drains an in-flight commit on disposal but rejects the next queued replay', async () => {
    const value = await setup();
    try {
      const first = value.logger.recordOwnerConnectionReplay(replay(1));
      await value.started;
      const second = value.logger.recordOwnerConnectionReplay(replay(2));
      let disposed = false;
      const disposal = value.logger.dispose().then(() => {
        disposed = true;
      });
      await Promise.resolve();
      expect(disposed).toBe(false);
      await expect(value.logger.recordOwnerConnectionReplay(replay(3))).resolves.toBe(false);
      value.release();
      await expect(first).resolves.toBe(true);
      await expect(second).resolves.toBe(false);
      await disposal;
      expect(value.write).toHaveBeenCalledOnce();
    } finally {
      value.release();
      await value.logger.dispose();
    }
  });

  it('retains the 64-commit admission cap while ordinary failure logging remains independent', async () => {
    const value = await setup();
    try {
      const first = value.logger.recordOwnerConnectionReplay(replay(1));
      await value.started;
      const queued = Array.from({ length: 63 }, (_, index) =>
        value.logger.recordOwnerConnectionReplay(replay(index + 2)),
      );
      await expect(value.logger.recordOwnerConnectionReplay(replay(65))).resolves.toBe(false);
      await expect(value.logger.recordStartupFailure('HELPER_STARTUP_UNAVAILABLE')).resolves.toBe(
        true,
      );
      value.release();
      expect(await Promise.all([first, ...queued])).toEqual(Array.from({ length: 64 }, () => true));
      expect(value.write).toHaveBeenCalledTimes(64);
    } finally {
      value.release();
      await value.logger.dispose();
    }
  });
});
