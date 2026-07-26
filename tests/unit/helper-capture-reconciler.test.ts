import { describe, expect, it, vi } from 'vitest';
import { HelperCaptureReconciler } from '../../app/src/main/echo/helper-capture-reconciler';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

describe('HelperCaptureReconciler', () => {
  it('reissues capture-off when native activation invalidates an in-flight acknowledgement', async () => {
    const firstDisable = deferred<unknown>();
    const secondDisable = deferred<unknown>();
    let disableCalls = 0;
    const setSessionCapture = vi.fn((active: boolean) => {
      if (active) return Promise.resolve({ active });
      disableCalls += 1;
      return disableCalls === 1 ? firstDisable.promise : secondDisable.promise;
    });
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    await reconciler.request(true, generation);

    reconciler.markNativeCaptureArmed();
    const firstRequest = reconciler.request(false, generation);
    await vi.waitFor(() => expect(disableCalls).toBe(1));
    reconciler.markNativeCaptureArmed();
    const secondRequest = reconciler.request(false, generation);
    firstDisable.resolve({ active: false });

    await vi.waitFor(() => expect(disableCalls).toBe(2));
    expect(onCaptureOff).not.toHaveBeenCalled();
    secondDisable.resolve({ active: false });
    await Promise.all([firstRequest, secondRequest]);

    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });

  it('joins an in-flight reconciliation for duplicate same-state requests', async () => {
    const enable = deferred<unknown>();
    const setSessionCapture = vi.fn(() => enable.promise);
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      vi.fn(),
    );
    const generation = reconciler.beginGeneration();

    const firstRequest = reconciler.request(true, generation);
    await vi.waitFor(() => expect(setSessionCapture).toHaveBeenCalledOnce());
    const duplicateRequest = reconciler.request(true, generation);
    enable.resolve({ active: true });
    await Promise.all([firstRequest, duplicateRequest]);

    expect(setSessionCapture).toHaveBeenCalledOnce();
  });

  it('ignores a stale reset rejection when the latest revision still wants capture off', async () => {
    const reset = deferred<unknown>();
    let disableCalls = 0;
    const setSessionCapture = vi.fn((active: boolean) => {
      if (active) return Promise.resolve({ active });
      disableCalls += 1;
      return disableCalls === 1
        ? Promise.reject(new Error('capture state unknown'))
        : Promise.resolve({ active });
    });
    const resetSessionCapture = vi.fn(() => reset.promise);
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const staleRequest = reconciler.request(false, generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    reconciler.markAppliedUnknown();
    const latestRequest = reconciler.request(false, generation);
    reset.reject(new Error('stale reset failure'));
    await Promise.all([staleRequest, latestRequest]);

    expect(disableCalls).toBe(2);
    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });

  it('ignores a stale reset rejection when the latest revision wants capture on', async () => {
    const reset = deferred<unknown>();
    const setSessionCapture = vi
      .fn<(active: boolean) => Promise<unknown>>()
      .mockRejectedValueOnce(new Error('capture state unknown'))
      .mockResolvedValue({ active: true });
    const resetSessionCapture = vi.fn(() => reset.promise);
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const staleRequest = reconciler.request(false, generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    const latestRequest = reconciler.request(true, generation);
    reset.reject(new Error('stale reset failure'));
    await Promise.all([staleRequest, latestRequest]);

    expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([false, true]);
    expect(reconciler.captureOffGuaranteed).toBe(false);
    expect(onCaptureOff).not.toHaveBeenCalled();
  });

  it.each([false, true])(
    'invalidates a stale reset when a newer generation wants active=%s',
    async (active) => {
      const reset = deferred<unknown>();
      let disableCalls = 0;
      const setSessionCapture = vi.fn((desired: boolean) => {
        if (desired) return Promise.resolve({ active: true });
        disableCalls += 1;
        return disableCalls === 1
          ? Promise.reject(new Error('capture state unknown'))
          : Promise.resolve({ active: false });
      });
      const resetSessionCapture = vi.fn(() => reset.promise);
      const onCaptureOff = vi.fn();
      const reconciler = new HelperCaptureReconciler(
        { setSessionCapture, resetSessionCapture },
        onCaptureOff,
      );
      const obsoleteGeneration = reconciler.beginGeneration();
      reconciler.markNativeCaptureArmed();
      const staleRequest = reconciler.request(false, obsoleteGeneration);
      await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

      const currentGeneration = reconciler.beginGeneration();
      const latestRequest = reconciler.request(active, currentGeneration);
      reset.reject(new Error('stale reset failure'));
      await Promise.all([staleRequest, latestRequest]);

      expect(setSessionCapture).toHaveBeenLastCalledWith(active);
      expect(reconciler.captureOffGuaranteed).toBe(!active);
      expect(onCaptureOff).toHaveBeenCalledTimes(active ? 0 : 1);
    },
  );

  it('ignores requests from an obsolete capture generation', async () => {
    const setSessionCapture = vi.fn(() => Promise.resolve());
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      vi.fn(),
    );
    const obsolete = reconciler.beginGeneration();
    reconciler.beginGeneration();

    await reconciler.request(true, obsolete);

    expect(setSessionCapture).not.toHaveBeenCalled();
  });

  it('retries capture-off after transient helper request capacity is released', async () => {
    const capacity = Object.assign(new Error('helper busy'), { code: 'request-capacity' });
    const setSessionCapture = vi
      .fn<(active: boolean) => Promise<unknown>>()
      .mockRejectedValueOnce(capacity)
      .mockResolvedValue({ active: false });
    const resetSessionCapture = vi.fn(() => Promise.resolve());
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();

    await reconciler.request(false, generation);

    expect(setSessionCapture).toHaveBeenCalledTimes(2);
    expect(resetSessionCapture).not.toHaveBeenCalled();
    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });

  it.each(['not-running', 'request-timeout', 'transport-error'] as const)(
    'leaves process recovery to helper supervision for %s failures',
    async (code) => {
      const error = Object.assign(new Error('helper unavailable'), { code });
      const resetSessionCapture = vi.fn(() => Promise.resolve());
      const reconciler = new HelperCaptureReconciler(
        {
          setSessionCapture: () => Promise.reject(error),
          resetSessionCapture,
        },
        vi.fn(),
      );
      const generation = reconciler.beginGeneration();
      reconciler.markNativeCaptureArmed();

      await expect(reconciler.request(false, generation)).rejects.toBe(error);
      expect(resetSessionCapture).not.toHaveBeenCalled();
      expect(reconciler.captureOffGuaranteed).toBe(false);
    },
  );

  it('never starts reset fallback when a pending disable fails after shutdown begins', async () => {
    const disable = deferred<unknown>();
    const resetSessionCapture = vi.fn(() => Promise.resolve());
    const reconciler = new HelperCaptureReconciler(
      {
        setSessionCapture: () => disable.promise,
        resetSessionCapture,
      },
      vi.fn(),
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const request = reconciler.request(false, generation);
    await Promise.resolve();

    reconciler.beginShutdown();
    disable.reject(new Error('late disable failure'));

    await expect(request).rejects.toThrow('late disable failure');
    expect(resetSessionCapture).not.toHaveBeenCalled();
    expect(reconciler.captureOffGuaranteed).toBe(false);
  });

  it('aborts reset fallback that was already in flight when shutdown begins', async () => {
    let resetSignal: AbortSignal | undefined;
    const resetSessionCapture = vi.fn(
      (signal?: AbortSignal) =>
        new Promise<unknown>((_resolve, reject) => {
          resetSignal = signal;
          signal?.addEventListener(
            'abort',
            () => reject(new DOMException('reset stopped', 'AbortError')),
            { once: true },
          );
        }),
    );
    const reconciler = new HelperCaptureReconciler(
      {
        setSessionCapture: () => Promise.reject(new Error('disable failed')),
        resetSessionCapture,
      },
      vi.fn(),
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const request = reconciler.request(false, generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    reconciler.beginShutdown();

    await expect(request).rejects.toThrow('disable failed');
    expect(resetSignal?.aborted).toBe(true);
    expect(reconciler.captureOffGuaranteed).toBe(false);
  });

  it('observes best-effort disable failures and allows a later recovery', async () => {
    let failing = true;
    const setSessionCapture = vi.fn((active: boolean) =>
      !active && failing
        ? Promise.reject(new Error('disable failed'))
        : Promise.resolve({ active }),
    );
    const resetSessionCapture = vi.fn(() =>
      failing ? Promise.reject(new Error('reset failed')) : Promise.resolve(),
    );
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();

    reconciler.requestBestEffort(false, generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());
    expect(reconciler.captureOffGuaranteed).toBe(false);

    failing = false;
    reconciler.markAppliedUnknown();
    await reconciler.request(false, generation);

    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });
});
