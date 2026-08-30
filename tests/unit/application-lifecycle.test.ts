import { readFileSync } from 'node:fs';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  StartupCancelledError,
  StartupCleanupStack,
  armAbsoluteShutdownWatchdog,
  createFatalStartupReport,
  reportLifecycleDiagnostics,
  runBoundedLifecycle,
  runSynchronousLifecycle,
} from '../../app/src/main/app/lifecycle';
afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('application lifecycle hardening', () => {
  it('rolls staged startup ownership back once in reverse acquisition order', async () => {
    const calls: string[] = [];
    const cleanup = new StartupCleanupStack();
    cleanup.add('history', () => {
      calls.push('history');
    });
    cleanup.add('worker', () => {
      calls.push('worker');
      return Promise.resolve();
    });
    cleanup.add('windows', () => {
      calls.push('windows');
    });

    expect(await cleanup.rollback()).toEqual([]);
    expect(await cleanup.rollback()).toEqual([]);
    expect(calls).toEqual(['windows', 'worker', 'history']);
  });

  it.each([1, 2, 3, 4])(
    'rolls back every acquired prefix when stage %i fails',
    async (acquired) => {
      const calls: string[] = [];
      const cleanup = new StartupCleanupStack();
      for (const name of ['persistence', 'worker', 'ipc', 'windows'].slice(0, acquired)) {
        cleanup.add(name, () => {
          calls.push(name);
        });
      }

      await cleanup.rollback();

      expect(calls).toEqual(
        ['persistence', 'worker', 'ipc', 'windows'].slice(0, acquired).reverse(),
      );
    },
  );

  it('stops and drains acquired IPC before rolling back its dependencies', async () => {
    const calls: string[] = [];
    let releaseInvocation!: () => void;
    const invocation = new Promise<void>((resolve) => {
      releaseInvocation = resolve;
    });
    const cleanup = new StartupCleanupStack();
    cleanup.add('dependency', () => {
      calls.push('dependency');
    });
    cleanup.add('ipc', async () => {
      calls.push('ipc:stop');
      await invocation;
      calls.push('ipc:drained');
    });

    const rollback = cleanup.rollback();
    await vi.waitFor(() => expect(calls).toEqual(['ipc:stop']));
    expect(calls).not.toContain('dependency');
    releaseInvocation();
    await rollback;
    expect(calls).toEqual(['ipc:stop', 'ipc:drained', 'dependency']);
  });

  it('continues after settled rejection but stops dependent cleanup after timeout', async () => {
    vi.useFakeTimers();
    const calls: string[] = [];
    const lifecycle = runBoundedLifecycle(
      'shutdown',
      [
        {
          name: 'recording',
          run: () => {
            calls.push('recording');
            throw new Error('https://secret.example C:\\Users\\canary token=secret-canary');
          },
        },
        { name: 'worker', run: () => new Promise<void>(() => undefined) },
        {
          name: 'persistence',
          run: () => {
            calls.push('persistence');
          },
        },
      ],
      90,
    );

    await vi.runAllTimersAsync();
    const diagnostics = await lifecycle;
    expect(calls).toEqual(['recording']);
    expect(diagnostics).toEqual([
      { phase: 'shutdown', step: 'recording', outcome: 'rejected' },
      { phase: 'shutdown', step: 'worker', outcome: 'timed-out' },
    ]);
    expect(JSON.stringify(diagnostics)).not.toMatch(/secret|https|Users|canary/i);
  });

  it('lets a step use the remaining aggregate deadline instead of an equal fraction', async () => {
    vi.useFakeTimers();
    const calls: string[] = [];
    const lifecycle = runBoundedLifecycle(
      'shutdown',
      [
        {
          name: 'producer',
          run: () =>
            new Promise<void>((resolve) => {
              setTimeout(() => {
                calls.push('producer');
                resolve();
              }, 1_000);
            }),
        },
        { name: 'dependent', run: () => void calls.push('dependent') },
      ],
      5_000,
    );

    await vi.advanceTimersByTimeAsync(1_000);
    await expect(lifecycle).resolves.toEqual([]);
    expect(calls).toEqual(['producer', 'dependent']);
  });

  it('forces process shutdown at the absolute deadline and supports settled cancellation', async () => {
    vi.useFakeTimers();
    const forceQuit = vi.fn();
    const deadline = Date.now() + 100;
    await vi.advanceTimersByTimeAsync(80);
    const watchdog = armAbsoluteShutdownWatchdog(deadline, forceQuit);

    await vi.advanceTimersByTimeAsync(19);
    expect(forceQuit).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(forceQuit).toHaveBeenCalledOnce();

    const cancelledForceQuit = vi.fn();
    const cancelled = armAbsoluteShutdownWatchdog(Date.now() + 100, cancelledForceQuit);
    cancelled.cancel();
    await vi.advanceTimersByTimeAsync(100);
    expect(cancelledForceQuit).not.toHaveBeenCalled();
    watchdog.cancel();
  });

  it('routes failed reset aborts through the canonical watchdog quit path', () => {
    const source = readFileSync('app/src/main/app/application.ts', 'utf8');
    const abortStart = source.indexOf('  #abortAfterFailedReset(');
    const abortEnd = source.indexOf('#acknowledgeDataReset(', abortStart);
    const abortMethod = source.slice(abortStart, abortEnd);
    const requestStart = source.indexOf('#requestQuit(options:');
    const requestEnd = source.indexOf('handleBeforeQuit(', requestStart);
    const requestMethod = source.slice(requestStart, requestEnd);

    expect(abortMethod).toContain('this.#requestQuit({ skipDependentShutdown: true })');
    expect(abortMethod).not.toMatch(/app\.(?:quit|exit)\(/u);
    expect(requestMethod).toContain('armAbsoluteShutdownWatchdog');
    expect(source.match(/app\.exit\(/gu)).toHaveLength(1);
  });

  it('honors an absolute deadline that started before lifecycle draining', async () => {
    vi.useFakeTimers();
    const deadline = Date.now() + 100;
    await vi.advanceTimersByTimeAsync(80);
    const lifecycle = runBoundedLifecycle(
      'shutdown',
      [{ name: 'remaining', run: () => new Promise<void>(() => undefined) }],
      5_000,
      { deadline },
    );
    await vi.advanceTimersByTimeAsync(20);
    await expect(lifecycle).resolves.toEqual([
      { phase: 'shutdown', step: 'remaining', outcome: 'timed-out' },
    ]);
  });

  it('starts no lifecycle step after an absolute deadline has expired', async () => {
    vi.useFakeTimers();
    const run = vi.fn();
    const deadline = Date.now() + 10;
    await vi.advanceTimersByTimeAsync(11);
    await expect(
      runBoundedLifecycle('shutdown', [{ name: 'late', run }], 5_000, { deadline }),
    ).resolves.toEqual([{ phase: 'shutdown', step: 'late', outcome: 'timed-out' }]);
    expect(run).not.toHaveBeenCalled();
  });

  it('prepares normal application renderers before helper activation and profile sync', () => {
    const source = readFileSync('app/src/main/app/application.ts', 'utf8');
    const renderersReady = source.indexOf('await windows.createAll()');
    const helperReady = source.indexOf('await helper.start()');
    const activationReady = source.indexOf('await echo.initialize()');

    expect(renderersReady).toBeGreaterThan(0);
    expect(helperReady).toBeGreaterThan(renderersReady);
    expect(activationReady).toBeGreaterThan(helperReady);
    expect(source.match(/state\.setHelperReadiness\(/gu)).toHaveLength(1);
    const bind = source.indexOf('sourceHarness.bindAndExposeTask6(task6Composition, echo)');
    const mediaActivation = source.indexOf('packagedMediaReady?.armAfterEchoBinding()');
    expect(bind).toBeGreaterThan(activationReady);
    expect(mediaActivation).toBeGreaterThan(bind);
  });

  it('distinguishes intentional startup cancellation from fatal startup failure', () => {
    expect(new StartupCancelledError()).toMatchObject({ name: 'StartupCancelledError' });
  });

  it('reports startup failure with stable public text and an opaque diagnostic identifier', () => {
    const canaries = [
      'secret-canary',
      'https://user:password@example.invalid/path?token=secret-canary',
      'C:\\Users\\private\\models',
      '/Users/private/models',
    ];
    const hostileError = new Error(canaries.join(' '));
    expect(hostileError.message).toContain(canaries[0]);
    const report = createFatalStartupReport();
    const serialized = JSON.stringify(report);

    expect(report).toMatchObject({
      code: 'STARTUP_FAILED',
      message: 'Talking Quill could not start. No diagnostic report was saved.',
    });
    expect(report.diagnosticId).toMatch(/^[0-9a-f-]{36}$/i);
    for (const canary of canaries) expect(serialized).not.toContain(canary);
  });

  it('continues synchronous final teardown after an injected disposer failure', () => {
    const calls: string[] = [];
    const diagnostics = runSynchronousLifecycle('shutdown', [
      {
        name: 'ipc',
        run: () => {
          calls.push('ipc');
          throw new Error('secret path C:\\private');
        },
      },
      {
        name: 'windows',
        run: () => {
          calls.push('windows');
        },
      },
      {
        name: 'history',
        run: () => {
          calls.push('history');
        },
      },
    ]);

    expect(calls).toEqual(['ipc', 'windows', 'history']);
    expect(diagnostics).toEqual([{ phase: 'shutdown', step: 'ipc', outcome: 'rejected' }]);
    expect(JSON.stringify(diagnostics)).not.toContain('private');
  });

  it('logs only structured lifecycle outcomes and opaque identifiers', () => {
    const error = vi.spyOn(console, 'error').mockImplementation(() => undefined);
    reportLifecycleDiagnostics(
      [{ phase: 'shutdown', step: 'vault', outcome: 'rejected' }],
      '00000000-0000-4000-8000-000000000000',
    );

    expect(error).toHaveBeenCalledWith('Talking Quill lifecycle cleanup incomplete', {
      diagnosticId: '00000000-0000-4000-8000-000000000000',
      diagnostics: [{ phase: 'shutdown', step: 'vault', outcome: 'rejected' }],
    });
  });
});
