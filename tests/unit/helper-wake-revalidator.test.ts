import { EventEmitter } from 'node:events';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { installHelperWakeRevalidator } from '../../app/src/main/helper/helper-wake-revalidator';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

async function flushMicrotasks(): Promise<void> {
  for (let flush = 0; flush < 6; flush += 1) await Promise.resolve();
}

afterEach(() => vi.useRealTimers());

describe('helper wake revalidation', () => {
  it('replaces a stale-ready helper with a fresh generation on resume', async () => {
    const source = new EventEmitter();
    let helperGeneration = 1;
    const staleReadyGeneration = helperGeneration;
    const recycle = vi.fn(() => {
      helperGeneration += 1;
      return Promise.resolve();
    });
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => true,
      recycle,
    });

    source.emit('resume');

    await vi.waitFor(() => expect(recycle).toHaveBeenCalledOnce());
    expect(staleReadyGeneration).toBe(1);
    expect(helperGeneration).toBe(2);
    dispose();
  });

  it('retains one wake through active work and recycles once all capture activity is idle', async () => {
    vi.useFakeTimers();
    const source = new EventEmitter();
    let phase: 'recording' | 'processing' | 'idle' = 'recording';
    let activationTest = false;
    let shortcutCapture = false;
    let helperGeneration = 1;
    const recycle = vi.fn(() => {
      helperGeneration += 1;
      return Promise.resolve();
    });
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => phase === 'idle' && !activationTest && !shortcutCapture,
      recycle,
    });

    source.emit('resume');
    await vi.advanceTimersByTimeAsync(1_000);
    expect(recycle).not.toHaveBeenCalled();

    phase = 'processing';
    await vi.advanceTimersByTimeAsync(1_000);
    expect(recycle).not.toHaveBeenCalled();

    phase = 'idle';
    activationTest = true;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(recycle).not.toHaveBeenCalled();

    activationTest = false;
    shortcutCapture = true;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(recycle).not.toHaveBeenCalled();

    shortcutCapture = false;
    await vi.advanceTimersByTimeAsync(1_000);
    await flushMicrotasks();
    expect(recycle).toHaveBeenCalledOnce();
    expect(helperGeneration).toBe(2);

    await vi.advanceTimersByTimeAsync(10_000);
    expect(recycle).toHaveBeenCalledOnce();
    dispose();
  });

  it('coalesces overlapping resume and unlock events into one trailing recycle', async () => {
    const source = new EventEmitter();
    const first = deferred<unknown>();
    const second = deferred<unknown>();
    const recycle = vi
      .fn<() => Promise<unknown>>()
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise);
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => true,
      recycle,
    });

    source.emit('resume');
    await vi.waitFor(() => expect(recycle).toHaveBeenCalledOnce());
    source.emit('unlock-screen');
    source.emit('resume');
    source.emit('unlock-screen');

    first.resolve(undefined);
    await vi.waitFor(() => expect(recycle).toHaveBeenCalledTimes(2));
    await flushMicrotasks();
    expect(recycle).toHaveBeenCalledTimes(2);

    second.resolve(undefined);
    dispose();
  });

  it('rechecks active-capture safety before queued and trailing recycles', async () => {
    const source = new EventEmitter();
    let safe = true;
    const inFlight = deferred<unknown>();
    const recycle = vi.fn(() => inFlight.promise);
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => safe,
      recycle,
    });

    source.emit('resume');
    safe = false;
    await flushMicrotasks();
    expect(recycle).not.toHaveBeenCalled();

    safe = true;
    source.emit('resume');
    await vi.waitFor(() => expect(recycle).toHaveBeenCalledOnce());
    source.emit('unlock-screen');
    safe = false;
    inFlight.resolve(undefined);
    await flushMicrotasks();
    expect(recycle).toHaveBeenCalledOnce();
    dispose();
  });

  it('cancels a pending unsafe wake when disposed', async () => {
    vi.useFakeTimers();
    const source = new EventEmitter();
    const recycle = vi.fn(() => Promise.resolve());
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => false,
      recycle,
    });

    source.emit('resume');
    dispose();
    await vi.advanceTimersByTimeAsync(10_000);

    expect(recycle).not.toHaveBeenCalled();
    expect(source.listenerCount('resume')).toBe(0);
    expect(source.listenerCount('unlock-screen')).toBe(0);
  });

  it('does not run queued recovery after disposal and removes both listeners once', async () => {
    const source = new EventEmitter();
    const recycle = vi.fn(() => Promise.resolve());
    const dispose = installHelperWakeRevalidator({
      source,
      isSafeToRevalidate: () => true,
      recycle,
    });

    source.emit('resume');
    dispose();
    dispose();
    await flushMicrotasks();

    expect(recycle).not.toHaveBeenCalled();
    expect(source.listenerCount('resume')).toBe(0);
    expect(source.listenerCount('unlock-screen')).toBe(0);
  });
});
