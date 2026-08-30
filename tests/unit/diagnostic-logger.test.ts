import { mkdir, readdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  DiagnosticLogger,
  type DiagnosticLoggerOptions,
} from '../../app/src/main/security/diagnostic-logger';
import { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { HelperRuntimeObservability } from '../../app/src/shared/helper/protocol';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const owned: string[] = [];
const ZERO_OBSERVABILITY = {
  keyboardOwner: {
    model: 'out_of_process',
    protocolVersion: 1,
    state: 'leased_disabled',
    instanceId: 'owner-instance-1',
    buildId: 'owner-build-1',
    leaseEpoch: 1,
    authenticated: true,
  },
  owner: {
    starts: 1,
    cleanExits: 0,
    abnormalExits: 0,
    singletonCollisions: 0,
    authAttempts: 1,
    authFailures: { crossUser: 0, wrongSession: 0, codeIdentity: 0, mac: 0, protocol: 0 },
    leaseAcquired: 1,
    leaseRenewed: 0,
    leaseExpired: 0,
    leaseDisconnected: 0,
    leaseReleasedNeutral: 0,
    leaseReleasedDraining: 0,
    drainDurationMsTotal: 0,
    drainDurationMsMax: 0,
    maintenancePostponed: 0,
    handoffSucceeded: 0,
    handoffFailed: 0,
    degraded: 0,
    hookRecoveries: 0,
  },
  registeredInput: {
    hookInstalled: 1,
    pumpAlive: 1,
    hcActionCallbacks: 0,
    physicalCallbacks: 0,
    physicalCallbacksFiltered: 0,
    registeredCandidateCallbacks: 0,
    registeredMatchCallbacks: 0,
    registeredReleaseCallbacks: 0,
    callbackChannelAccepted: 0,
    callbackChannelRejected: 0,
    adapterDequeued: 0,
    ownerAdmitted: 0,
    ownerFlushed: 0,
    ownerRejected: 0,
    gatewayReceived: 0,
    v10NotificationAccepted: 0,
    electronReceived: 0,
    observationAccepted: 0,
  },
  keyboardCapture: {
    runtimeRollbackActive: false,
    developmentDisabled: false,
    activationEnableRequestsBlocked: 0,
    sessionCaptureRequestsBlocked: 0,
    shutdownOwnershipDeadlines: 0,
    terminalDisablements: 0,
  },
  transactions: {
    started: 0,
    committed: 0,
    replayed: 0,
    cancelled: 1,
    journalHighWater: 0,
    cancellationReasons: {
      invalidContinuation: 0,
      modifierChanged: 0,
      altGr: 0,
      journalOverflow: 0,
      configurationReplaced: 0,
      revisionMismatch: 0,
      gateClosed: 0,
      shutdown: 1,
      helperDisconnected: 0,
      secureDesktop: 0,
      timeout: 0,
      activationDeliveryFailed: 0,
      neutralizationFailed: 0,
      replayFailed: 0,
      effectProtocolViolation: 0,
      physicalStateMismatch: 0,
      targetChanged: 0,
    },
  },
  replay: { attempted: 0, succeeded: 0, partial: 0, failed: 0 },
  dummy: { attempted: 0, succeeded: 0, partial: 0, failed: 0 },
  paste: {
    attempted: 0,
    submitted: 0,
    targetValidationFallback: 0,
    nativeWaitDurationMsTotal: 75,
    nativeWaitDurationMsMax: 75,
    modifierTimeouts: 0,
    failures: {
      permissionDenied: 0,
      secureInput: 0,
      conflictingModifiers: 0,
      osRejected: 0,
      unavailable: 0,
      indeterminate: 0,
    },
  },
} satisfies HelperRuntimeObservability;

afterEach(async () => {
  await Promise.all(owned.splice(0).map((path) => removeTestDirectory(path)));
});

async function setup(enabled: boolean, options: DiagnosticLoggerOptions = {}) {
  const root = await createTestDirectory('diagnostic-logger');
  owned.push(root);
  const settings = new SettingsStore(join(root, 'settings.json'));
  await settings.initialize();
  if (enabled) await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
  let now = 1;
  const logs = join(root, 'logs');
  const logger = new DiagnosticLogger(settings, logs, {
    ...options,
    now: () => now++,
  });
  await logger.initialize();
  return { root, logs, settings, logger };
}

function ownerReplay(
  count: number,
  options: {
    readonly journalId?: string;
    readonly journalNonce?: string;
    readonly streamId?: string;
    readonly processGeneration?: string;
    readonly operation?: 'lease.renew' | 'health.get';
    readonly category?: 'disconnected' | 'transport';
  } = {},
) {
  return {
    event: 'helper.owner.connection.replay',
    journalId: options.journalId ?? '01'.repeat(32),
    journalNonce: options.journalNonce ?? '23'.repeat(32),
    streamId: options.streamId ?? '45'.repeat(32),
    processGeneration: options.processGeneration ?? '1',
    category: options.category ?? 'disconnected',
    operation: options.operation ?? 'lease.renew',
    correlationStatus: 'pending',
    healthRefresh: 'not_attempted',
    transportStatus: options.category === 'transport' ? 'error' : 'eof',
    ownerProcessState: 'running',
    count: String(count),
    counterOverflow: false,
    durable: true,
    durabilityFailures: '0',
    writerStartFailures: '0',
    synchronizationRecoveries: '0',
  } as const;
}

describe('DiagnosticLogger', () => {
  it('never commits or acknowledges owner replay before aggregate initialization', async () => {
    const root = await createTestDirectory('diagnostic-uninitialized-replay');
    owned.push(root);
    const settings = new SettingsStore(join(root, 'settings.json'));
    await settings.initialize();
    const logs = join(root, 'logs');
    const logger = new DiagnosticLogger(settings, logs);
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(1))).resolves.toBe(false);
    await Promise.allSettled(
      Array.from({ length: 257 }, () =>
        logger.record('application.started', {
          component: 'application',
          outcome: 'ready',
          appVersion: '1.0.0',
          runtimeVersion: '1.0.0',
        }),
      ),
    );
    await expect(stat(join(logs, 'owner-connection-counts.json'))).rejects.toMatchObject({
      code: 'ENOENT',
    });
    await settings.flush();
  });

  it('persists ordinary details only while the debug setting is enabled', async () => {
    const { logs, settings, logger } = await setup(false);
    await expect(
      logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        appVersion: '1.2.3',
        runtimeVersion: '1.2.3',
      }),
    ).resolves.toBe(false);
    await expect(readdir(logs)).rejects.toMatchObject({ code: 'ENOENT' });
    await expect(
      logger.record('helper.runtime.snapshot', {
        component: 'helper',
        outcome: 'runtime',
        observability: ZERO_OBSERVABILITY,
      }),
    ).resolves.toBe(false);

    await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
    await expect(
      logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        appVersion: '1.2.3',
        runtimeVersion: '1.2.3',
      }),
    ).resolves.toBe(true);
    await logger.record('helper.readiness.changed', {
      component: 'helper',
      outcome: 'unavailable',
      reason: 'owner-auth-failed',
    });
    await logger.record('helper.owner.connection', {
      component: 'helper',
      outcome: 'failure',
      ownerErrorCategory: 'disconnected',
      ownerOperation: 'lease.renew',
      correlationStatus: 'pending',
      healthRefresh: 'not_attempted',
      transportStatus: 'eof',
      ownerProcessState: 'running',
    });
    await logger.record('application.activation', {
      component: 'application',
      outcome: 'requested',
      activationSource: 'second_instance',
      activationSequence: 1,
      restoreHandlerReached: true,
      showMainReached: true,
    });
    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('application.started');
    expect(source).toContain('"appVersion":"1.2.3"');
    expect(source).toContain('helper.readiness.changed');
    expect(source).toContain('owner-auth-failed');
    expect(source).toContain('helper.owner.connection');
    expect(source).toContain('"ownerProcessState":"running"');
    expect(source).toContain('"ownerOperation":"lease.renew"');
    expect(source).toContain('application.activation');
    expect(source).toContain('"activationSource":"second_instance"');
    expect(source).not.toMatch(
      /transcript|authorization|"request"|"response"|body|audio|screenshot/i,
    );

    await settings.update({ privacy: { diagnosticLoggingEnabled: false } });
    const before = await stat(join(logs, 'diagnostic.jsonl'));
    await logger.record('application.stopping', {
      component: 'application',
      outcome: 'requested',
    });
    await expect(
      logger.record('helper.runtime.snapshot', {
        component: 'helper',
        outcome: 'runtime',
        observability: ZERO_OBSERVABILITY,
      }),
    ).resolves.toBe(false);
    const after = await stat(join(logs, 'diagnostic.jsonl'));
    expect(after.size).toBe(before.size);
    await logger.dispose();
  });

  it('keeps owner and lifecycle details out while disabled and admits only failure codes', async () => {
    const { logs, logger } = await setup(false);
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(1))).resolves.toBe(false);
    await expect(
      logger.record('helper.startup.failure', { code: 'HELPER_STARTUP_UNAVAILABLE' }),
    ).resolves.toBe(false);
    await expect(
      logger.record('application.activation', {
        component: 'application',
        outcome: 'requested',
        activationSource: 'second_instance',
        activationSequence: 1,
        restoreHandlerReached: true,
        showMainReached: true,
      }),
    ).resolves.toBe(false);
    await expect(logger.recordStartupFailure('HELPER_STARTUP_UNAVAILABLE')).resolves.toBe(true);
    await logger.dispose();
    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('HELPER_STARTUP_UNAVAILABLE');
    expect(source).not.toMatch(/activation|second_instance|ownerOperation|ownerProcessState/u);
    await expect(stat(join(logs, 'owner-connection-counts.json'))).rejects.toMatchObject({
      code: 'ENOENT',
    });
  });

  it('always records bounded content-free helper startup failures', async () => {
    const { logs, logger } = await setup(false);
    const write = logger.recordStartupFailure('HELPER_STARTUP_UNAVAILABLE');
    const dispose = logger.dispose();
    await expect(write).resolves.toBe(true);
    await dispose;
    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('helper.startup.failure');
    expect(source).toContain('HELPER_STARTUP_UNAVAILABLE');
    expect(source).not.toMatch(/transcript|shortcut|keystroke|token|secret|audio/i);
  });

  it('always records bounded content-free helper runtime failures', async () => {
    const { logs, logger } = await setup(false);
    await expect(logger.recordOperationalFailure('HELPER_RUNTIME_UNAVAILABLE')).resolves.toBe(true);
    await logger.dispose();
    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('helper.operational.failure');
    expect(source).toContain('HELPER_RUNTIME_UNAVAILABLE');
    expect(source).not.toMatch(/transcript|shortcut|keystroke|token|secret|audio/i);
  });

  it('recovers operational logging after an invalid log path is repaired', async () => {
    const root = await createTestDirectory('diagnostic-init-failure');
    owned.push(root);
    const settings = new SettingsStore(join(root, 'settings.json'));
    await settings.initialize();
    await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
    const logs = join(root, 'logs');
    await writeFile(logs, 'blocks directory creation');
    const logger = new DiagnosticLogger(settings, logs);
    await expect(logger.initialize()).rejects.toThrow();

    await settings.update({ privacy: { diagnosticLoggingEnabled: false } });
    await rm(logs);
    await expect(logger.recordOperationalFailure('HELPER_RUNTIME_INCOMPATIBLE')).resolves.toBe(
      true,
    );
    await expect(readdir(logs)).resolves.toContain('diagnostic.jsonl');
    await logger.dispose();
  });

  it('writes strict helper aggregates without admitting keys or target tokens', async () => {
    const { logs, logger } = await setup(true);
    await logger.record('helper.runtime.snapshot', {
      component: 'helper',
      outcome: 'shutdown',
      observability: ZERO_OBSERVABILITY,
    });
    expect(() =>
      logger.record('helper.runtime.snapshot', {
        component: 'helper',
        outcome: 'shutdown',
        observability: { ...ZERO_OBSERVABILITY, targetToken: 'opaque-private-value' },
      } as never),
    ).toThrow();
    expect(() =>
      logger.record('helper.runtime.snapshot', {
        component: 'helper',
        outcome: 'shutdown',
      }),
    ).toThrow('does not match');
    expect(() =>
      logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        observability: ZERO_OBSERVABILITY,
      }),
    ).toThrow('does not match');

    const source = await readFile(join(logs, 'diagnostic.jsonl'), 'utf8');
    expect(source).toContain('helper.runtime.snapshot');
    expect(source).toContain('"nativeWaitDurationMsTotal":75');
    expect(source).not.toMatch(
      /opaque-private-value|owner-instance-1|owner-build-1|targetToken|shortcut|keyStream/,
    );
    expect(source).toContain('"leaseEpoch":"[REDACTED]"');
    await logger.dispose();
  });

  it('rejects non-allowlisted metadata instead of attempting to redact user content', async () => {
    const { logs, logger } = await setup(true);
    expect(() =>
      logger.record('application.started', {
        component: 'application',
        transcript: 'private words',
      } as never),
    ).toThrow();
    await expect(readdir(logs)).resolves.toEqual([]);
    await logger.dispose();
  });

  it('rotates bounded files and keeps restrictive file permissions where supported', async () => {
    const { logs, logger } = await setup(true, { maxBytes: 1_024 });
    for (let index = 0; index < 40; index += 1) {
      await logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        code: `READY_${String(index)}`,
      });
    }
    await logger.dispose();
    const files = (await readdir(logs)).sort();
    expect(files).toEqual([
      'diagnostic.jsonl',
      'diagnostic.jsonl.1',
      'diagnostic.jsonl.2',
      'diagnostic.jsonl.3',
    ]);
    for (const file of files) {
      const metadata = await stat(join(logs, file));
      expect(metadata.size).toBeLessThanOrEqual(1_024);
      if (process.platform !== 'win32') expect(metadata.mode & 0o777).toBe(0o600);
    }
  });

  it('rejects sentinel and malformed live identities without mutating totals or high-water', async () => {
    const { logs, logger } = await setup(true);
    for (const sentinel of ['0'.repeat(64), 'f'.repeat(64)]) {
      for (const field of ['journalId', 'journalNonce', 'streamId'] as const) {
        expect(() =>
          logger.recordOwnerConnectionReplay(ownerReplay(9, { [field]: sentinel })),
        ).toThrow();
      }
    }
    expect(() =>
      logger.recordOwnerConnectionReplay(ownerReplay(9, { journalId: '01'.repeat(31) })),
    ).toThrow();
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(1))).resolves.toBe(true);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; journals: { highWater: { value: string }[] }[] };
    expect(checkpoint.acceptedDisconnects).toBe('1');
    expect(checkpoint.journals).toHaveLength(1);
    expect(checkpoint.journals[0]?.highWater).toEqual([expect.objectContaining({ value: '1' })]);
  });

  it('rejects sentinel checkpoint identities before migration or state mutation', async () => {
    const root = await createTestDirectory('diagnostic-logger-sentinel-checkpoint');
    owned.push(root);
    const settings = new SettingsStore(join(root, 'settings.json'));
    await settings.initialize();
    await settings.update({ privacy: { diagnosticLoggingEnabled: true } });
    const logs = join(root, 'logs');
    await mkdir(logs, { recursive: true });
    const checkpointPath = join(logs, 'owner-connection-counts.json');
    const base = {
      version: 2,
      updatedAt: 1,
      acceptedDisconnects: '9',
      duplicateCumulativeRecords: '0',
      persistenceFailures: '0',
      nonOwnerQueueOverflows: '0',
      journalCapacityRejects: '0',
      streamIdentityCollisions: '0',
      helperCounterOverflowDimensions: [],
      dimensions: [],
      journals: [
        {
          journalId: '01'.repeat(32),
          journalNonce: '23'.repeat(32),
          durabilityFailures: '0',
          writerStartFailures: '0',
          synchronizationRecoveries: '0',
          highWater: [],
        },
      ],
      streams: [
        {
          journalId: '01'.repeat(32),
          journalNonce: '23'.repeat(32),
          processGeneration: '1',
          streamId: '45'.repeat(32),
          lastSeen: 1,
        },
      ],
    };
    const logger = new DiagnosticLogger(settings, logs);
    for (const sentinel of ['0'.repeat(64), 'f'.repeat(64)]) {
      for (const candidate of [
        { ...base, journals: [{ ...base.journals[0], journalId: sentinel }] },
        { ...base, journals: [{ ...base.journals[0], journalNonce: sentinel }] },
        { ...base, streams: [{ ...base.streams[0], streamId: sentinel }] },
      ]) {
        await writeFile(checkpointPath, JSON.stringify(candidate));
        await expect(logger.initialize()).rejects.toThrow();
      }
    }
    await writeFile(
      checkpointPath,
      JSON.stringify({
        ...base,
        streams: [{ ...base.streams[0], streamId: '45'.repeat(31) }],
      }),
    );
    await expect(logger.initialize()).rejects.toThrow();

    await writeFile(
      checkpointPath,
      JSON.stringify({
        version: 1,
        updatedAt: 2,
        acceptedDisconnects: '0',
        duplicateCumulativeRecords: '0',
        persistenceFailures: '0',
        nonOwnerQueueOverflows: '0',
        helperCounterOverflowDimensions: [],
        dimensions: [],
      }),
    );
    await logger.initialize();
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(1))).resolves.toBe(true);
    await logger.dispose();
    const migrated = JSON.parse(await readFile(checkpointPath, 'utf8')) as {
      acceptedDisconnects: string;
      journals: { journalId: string; journalNonce: string }[];
      streams: { streamId: string }[];
    };
    expect(migrated.acceptedDisconnects).toBe('1');
    expect(migrated.journals).toEqual([
      expect.objectContaining({ journalId: '01'.repeat(32), journalNonce: '23'.repeat(32) }),
    ]);
    expect(migrated.streams).toEqual([expect.objectContaining({ streamId: '45'.repeat(32) })]);
  });

  it('retains independent journal high-water state for A40, B5, A40 interleaving', async () => {
    const { logs, logger } = await setup(true);
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(40))).resolves.toBe(true);
    await expect(
      logger.recordOwnerConnectionReplay(
        ownerReplay(5, { journalId: '67'.repeat(32), journalNonce: '89'.repeat(32) }),
      ),
    ).resolves.toBe(true);
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(40))).resolves.toBe(true);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; duplicateCumulativeRecords: string; journals: unknown[] };
    expect(checkpoint.acceptedDisconnects).toBe('45');
    expect(checkpoint.duplicateCumulativeRecords).toBe('1');
    expect(checkpoint.journals).toHaveLength(2);
  });

  it('does not recount when Electron restarts while the same helper journal lives', async () => {
    const first = await setup(true);
    await first.logger.recordOwnerConnectionReplay(ownerReplay(40));
    await first.logger.dispose();
    const settings = new SettingsStore(join(first.root, 'settings.json'));
    await settings.initialize();
    const restarted = new DiagnosticLogger(settings, first.logs);
    await restarted.initialize();
    await restarted.recordOwnerConnectionReplay(ownerReplay(40));
    await restarted.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(first.logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; duplicateCumulativeRecords: string };
    expect(checkpoint.acceptedDisconnects).toBe('40');
    expect(checkpoint.duplicateCumulativeRecords).toBe('1');
  });

  it('fails closed on one generation with conflicting stream IDs', async () => {
    const { logs, logger } = await setup(true);
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(40))).resolves.toBe(true);
    await expect(
      logger.recordOwnerConnectionReplay(ownerReplay(41, { streamId: 'ab'.repeat(32) })),
    ).resolves.toBe(false);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; streamIdentityCollisions: string };
    expect(checkpoint.acceptedDisconnects).toBe('40');
    expect(checkpoint.streamIdentityCollisions).toBe('1');
  });

  it('scopes forced stream-ID collisions by durable journal identity and generation', async () => {
    const { logs, logger } = await setup(true);
    const streamId = 'cd'.repeat(32);
    await logger.recordOwnerConnectionReplay(ownerReplay(40, { streamId }));
    await logger.recordOwnerConnectionReplay(
      ownerReplay(5, {
        journalId: 'ef'.repeat(32),
        journalNonce: '10'.repeat(32),
        streamId,
      }),
    );
    await logger.recordOwnerConnectionReplay(ownerReplay(45, { streamId, processGeneration: '2' }));
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; streams: unknown[] };
    expect(checkpoint.acceptedDisconnects).toBe('50');
    expect(checkpoint.streams).toHaveLength(3);
  });

  it('leaves a replay unacknowledged when the sink crashes before checkpoint commit', async () => {
    const first = await setup(true, {
      writeOwnerAggregate: () => Promise.reject(new Error('injected crash window')),
    });
    await expect(first.logger.recordOwnerConnectionReplay(ownerReplay(40))).rejects.toThrow();
    await first.logger.dispose();
    const settings = new SettingsStore(join(first.root, 'settings.json'));
    await settings.initialize();
    const restarted = new DiagnosticLogger(settings, first.logs);
    await restarted.initialize();
    await expect(restarted.recordOwnerConnectionReplay(ownerReplay(40))).resolves.toBe(true);
    await restarted.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(first.logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string };
    expect(checkpoint.acceptedDisconnects).toBe('40');
  });

  it('retries disk-full replay without double counting after storage recovery', async () => {
    let attempts = 0;
    const { logs, logger } = await setup(true, {
      writeOwnerAggregate: async (path, contents) => {
        attempts += 1;
        if (attempts === 1) throw new Error('ENOSPC');
        await writeFile(path, contents);
      },
    });
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(73))).rejects.toThrow('ENOSPC');
    await expect(logger.recordOwnerConnectionReplay(ownerReplay(73))).resolves.toBe(true);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; duplicateCumulativeRecords: string };
    expect(checkpoint.acceptedDisconnects).toBe('73');
    expect(checkpoint.duplicateCumulativeRecords).toBe('1');
  });

  it('retains replay proof while ordinary logs rotate and after restart', async () => {
    const first = await setup(true, { maxBytes: 1_024 });
    await first.logger.recordOwnerConnectionReplay(ownerReplay(73));
    for (let index = 0; index < 40; index += 1) {
      await first.logger.record('application.started', {
        component: 'application',
        outcome: 'ready',
        code: `ROTATE_${String(index)}`,
      });
    }
    await first.logger.dispose();
    const settings = new SettingsStore(join(first.root, 'settings.json'));
    await settings.initialize();
    const restarted = new DiagnosticLogger(settings, first.logs);
    await restarted.initialize();
    await restarted.recordOwnerConnectionReplay(
      ownerReplay(73, { streamId: 'fe'.repeat(32), processGeneration: '2' }),
    );
    await restarted.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(first.logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string };
    expect(checkpoint.acceptedDisconnects).toBe('73');
    expect(
      (await readdir(first.logs)).filter((file) => file.startsWith('diagnostic.jsonl')),
    ).toHaveLength(4);
  });

  it('bounds journal identity retention without unsafe high-water collection', async () => {
    const { logs, logger } = await setup(true);
    for (let index = 1; index <= 32; index += 1) {
      const byte = index.toString(16).padStart(2, '0');
      await expect(
        logger.recordOwnerConnectionReplay(
          ownerReplay(1, { journalId: byte.repeat(32), journalNonce: 'aa'.repeat(32) }),
        ),
      ).resolves.toBe(true);
    }
    await expect(
      logger.recordOwnerConnectionReplay(
        ownerReplay(1, { journalId: '21'.repeat(32), journalNonce: 'bb'.repeat(32) }),
      ),
    ).resolves.toBe(false);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; journalCapacityRejects: string; journals: unknown[] };
    expect(checkpoint.acceptedDisconnects).toBe('32');
    expect(checkpoint.journalCapacityRejects).toBe('1');
    expect(checkpoint.journals).toHaveLength(32);
  });

  it('collects audit-only stream metadata at 256 without collecting journal high-water', async () => {
    const { logs, logger } = await setup(true);
    for (let generation = 1; generation <= 300; generation += 1) {
      const streamId = generation.toString(16).padStart(64, '0');
      await logger.recordOwnerConnectionReplay(
        ownerReplay(1, {
          processGeneration: String(generation),
          streamId,
        }),
      );
    }
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { acceptedDisconnects: string; streams: unknown[]; journals: { highWater: unknown[] }[] };
    expect(checkpoint.acceptedDisconnects).toBe('1');
    expect(checkpoint.streams).toHaveLength(256);
    expect(checkpoint.journals[0]?.highWater).toHaveLength(1);
  });

  it('bounds the ordinary promise queue and durably accounts for rejected overflow', async () => {
    const { logs, logger } = await setup(true);
    const writes = Array.from({ length: 1_000 }, (_, index) =>
      logger
        .record('application.started', {
          component: 'application',
          outcome: 'ready',
          code: `BURST_${String(index)}`,
        })
        .catch(() => false),
    );
    await Promise.all(writes);
    await logger.dispose();
    const checkpoint = JSON.parse(
      await readFile(join(logs, 'owner-connection-counts.json'), 'utf8'),
    ) as { nonOwnerQueueOverflows: string };
    expect(BigInt(checkpoint.nonOwnerQueueOverflows)).toBeGreaterThan(0n);
  });

  it('rejects secret-bearing aggregate fields before they reach durable state', async () => {
    const { logs, logger } = await setup(false);
    expect(() =>
      logger.recordOwnerConnectionReplay({
        ...ownerReplay(1),
        token: 'must-not-persist',
      }),
    ).toThrow();
    await logger.dispose();
    await expect(readdir(logs)).rejects.toMatchObject({ code: 'ENOENT' });
  });
});
