import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  StartupCancelledError,
  StartupCleanupStack,
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

  it('attempts every cleanup and reports rejection and timeout without raw errors', async () => {
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
    expect(calls).toEqual(['recording', 'persistence']);
    expect(diagnostics).toEqual([
      { phase: 'shutdown', step: 'recording', outcome: 'rejected' },
      { phase: 'shutdown', step: 'worker', outcome: 'timed-out' },
    ]);
    expect(JSON.stringify(diagnostics)).not.toMatch(/secret|https|Users|canary/i);
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
