import { describe, expect, it, vi } from 'vitest';
import {
  prepareResetSafely,
  ResetPreparationError,
} from '../../app/src/main/data/reset-preparation';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((done) => {
    resolve = done;
  });
  return { promise, resolve };
}

describe('atomic reset preparation', () => {
  it('quits and relaunches without reset when journal preparation fails after quiesce', async () => {
    const order: string[] = [];
    const onAbort = vi.fn();
    await expect(
      prepareResetSafely({
        journal: {
          prepareReset: () => {
            order.push('journal');
            return Promise.reject(new Error('disk unavailable'));
          },
          cancelPreparedReset: () => {
            order.push('cancel-journal');
            return Promise.resolve();
          },
        },
        quiesce: () => order.push('quiesce'),
        criticalSteps: [
          {
            name: 'must-not-run',
            run: () => {
              order.push('drain');
            },
          },
        ],
        timeoutMs: 100,
        onAbort,
      }),
    ).rejects.toBeInstanceOf(ResetPreparationError);
    expect(order).toEqual(['quiesce', 'journal', 'cancel-journal']);
    expect(onAbort).toHaveBeenCalledExactlyOnceWith(true);
  });

  it('does not acknowledge and preserves dependency order until critical drains settle', async () => {
    const first = deferred();
    const second = deferred();
    let settled = false;
    const started: string[] = [];
    const preparation = prepareResetSafely({
      journal: { prepareReset: vi.fn(), cancelPreparedReset: vi.fn() },
      quiesce: vi.fn(),
      criticalSteps: [
        {
          name: 'first',
          run: () => {
            started.push('first');
            return first.promise;
          },
        },
        {
          name: 'second',
          run: () => {
            started.push('second');
            return second.promise;
          },
        },
      ],
      timeoutMs: 1_000,
      onAbort: vi.fn(),
    }).then(() => {
      settled = true;
    });
    await vi.waitFor(() => expect(started).toEqual(['first']));
    first.resolve();
    await vi.waitFor(() => expect(started).toEqual(['first', 'second']));
    expect(settled).toBe(false);
    second.resolve();
    await preparation;
    expect(settled).toBe(true);
  });

  it('stops before dependent stores when a producer drain fails', async () => {
    const store = vi.fn();
    await expect(
      prepareResetSafely({
        journal: { prepareReset: vi.fn(), cancelPreparedReset: vi.fn() },
        quiesce: vi.fn(),
        criticalSteps: [
          { name: 'producer', run: () => Promise.reject(new Error('still active')) },
          { name: 'store', run: store },
        ],
        timeoutMs: 100,
        onAbort: vi.fn(),
      }),
    ).rejects.toMatchObject({
      diagnostics: [{ phase: 'shutdown', step: 'producer', outcome: 'rejected' }],
    });
    expect(store).not.toHaveBeenCalled();
  });

  it('cancels the journal and refuses reset relaunch when a critical drain rejects', async () => {
    const cancel = vi.fn();
    const onAbort = vi.fn();
    await expect(
      prepareResetSafely({
        journal: { prepareReset: vi.fn(), cancelPreparedReset: cancel },
        quiesce: vi.fn(),
        criticalSteps: [
          { name: 'settings', run: async () => Promise.reject(new Error('flush failed')) },
        ],
        timeoutMs: 100,
        onAbort,
      }),
    ).rejects.toMatchObject({
      diagnostics: [{ phase: 'shutdown', step: 'settings', outcome: 'rejected' }],
    });
    expect(cancel).toHaveBeenCalledOnce();
    expect(onAbort).toHaveBeenCalledExactlyOnceWith(true);
  });

  it('preserves drain diagnostics when journal cancellation also fails', async () => {
    const onAbort = vi.fn();
    const preparation = prepareResetSafely({
      journal: {
        prepareReset: vi.fn(),
        cancelPreparedReset: () => Promise.reject(new Error('journal cleanup failed')),
      },
      quiesce: vi.fn(),
      criticalSteps: [{ name: 'settings', run: () => Promise.reject(new Error('flush failed')) }],
      timeoutMs: 100,
      onAbort,
    });

    let rejection: unknown;
    try {
      await preparation;
    } catch (error: unknown) {
      rejection = error;
    }
    expect(rejection).toBeInstanceOf(ResetPreparationError);
    if (!(rejection instanceof ResetPreparationError)) throw new Error('Expected rejection');
    expect(rejection.diagnostics).toEqual([
      { phase: 'shutdown', step: 'settings', outcome: 'rejected' },
    ]);
    expect(rejection.cause).toBeInstanceOf(AggregateError);
    expect(onAbort).toHaveBeenCalledExactlyOnceWith(false);
  });
});
