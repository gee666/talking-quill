import { describe, expect, it, vi } from 'vitest';
import { HelperCaptureReconciler } from '../../app/src/main/echo/helper-capture-reconciler';
import type { HelperSessionCaptureMode } from '../../app/src/shared/helper/protocol';

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
    const setSessionCapture = vi.fn((mode: HelperSessionCaptureMode) => {
      if (mode !== 'off') return Promise.resolve({ mode });
      disableCalls += 1;
      return disableCalls === 1 ? firstDisable.promise : secondDisable.promise;
    });
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    await reconciler.request('recording', generation);

    reconciler.markNativeCaptureArmed();
    const firstRequest = reconciler.request('off', generation);
    await vi.waitFor(() => expect(disableCalls).toBe(1));
    reconciler.markNativeCaptureArmed();
    const secondRequest = reconciler.request('off', generation);
    firstDisable.resolve({ mode: 'off' });

    await vi.waitFor(() => expect(disableCalls).toBe(2));
    expect(onCaptureOff).not.toHaveBeenCalled();
    secondDisable.resolve({ mode: 'off' });
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

    const firstRequest = reconciler.request('recording', generation);
    await vi.waitFor(() => expect(setSessionCapture).toHaveBeenCalledOnce());
    const duplicateRequest = reconciler.request('recording', generation);
    enable.resolve({ mode: 'recording' });
    await Promise.all([firstRequest, duplicateRequest]);

    expect(setSessionCapture).toHaveBeenCalledOnce();
  });

  it('ignores a stale reset rejection when the latest revision still wants capture off', async () => {
    const reset = deferred<unknown>();
    let disableCalls = 0;
    const setSessionCapture = vi.fn((mode: HelperSessionCaptureMode) => {
      if (mode !== 'off') return Promise.resolve({ mode });
      disableCalls += 1;
      return disableCalls === 1
        ? Promise.reject(new Error('capture state unknown'))
        : Promise.resolve({ mode });
    });
    const resetSessionCapture = vi.fn(() => reset.promise);
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const staleRequest = reconciler.request('off', generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    reconciler.markAppliedUnknown();
    const latestRequest = reconciler.request('off', generation);
    reset.reject(new Error('stale reset failure'));
    await Promise.all([staleRequest, latestRequest]);

    expect(disableCalls).toBe(2);
    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });

  it('ignores a stale reset rejection when the latest revision wants capture on', async () => {
    const reset = deferred<unknown>();
    const setSessionCapture = vi
      .fn<(mode: HelperSessionCaptureMode) => Promise<unknown>>()
      .mockRejectedValueOnce(new Error('capture state unknown'))
      .mockResolvedValue({ mode: 'recording' });
    const resetSessionCapture = vi.fn(() => reset.promise);
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();
    const staleRequest = reconciler.request('off', generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    const latestRequest = reconciler.request('recording', generation);
    reset.reject(new Error('stale reset failure'));
    await Promise.all([staleRequest, latestRequest]);

    expect(setSessionCapture.mock.calls.map(([mode]) => mode)).toEqual(['off', 'recording']);
    expect(reconciler.captureOffGuaranteed).toBe(false);
    expect(onCaptureOff).not.toHaveBeenCalled();
  });

  it.each(['off', 'cancel-only'] as const)(
    'invalidates a stale reset when a newer generation wants mode=%s',
    async (mode) => {
      const reset = deferred<unknown>();
      let disableCalls = 0;
      const setSessionCapture = vi.fn((desired: HelperSessionCaptureMode) => {
        if (desired !== 'off') return Promise.resolve({ mode: desired });
        disableCalls += 1;
        return disableCalls === 1
          ? Promise.reject(new Error('capture state unknown'))
          : Promise.resolve({ mode: 'off' });
      });
      const resetSessionCapture = vi.fn(() => reset.promise);
      const onCaptureOff = vi.fn();
      const reconciler = new HelperCaptureReconciler(
        { setSessionCapture, resetSessionCapture },
        onCaptureOff,
      );
      const obsoleteGeneration = reconciler.beginGeneration();
      reconciler.markNativeCaptureArmed();
      const staleRequest = reconciler.request('off', obsoleteGeneration);
      await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

      const currentGeneration = reconciler.beginGeneration();
      const latestRequest = reconciler.request(mode, currentGeneration);
      reset.reject(new Error('stale reset failure'));
      await Promise.all([staleRequest, latestRequest]);

      expect(setSessionCapture).toHaveBeenLastCalledWith(mode);
      expect(reconciler.captureOffGuaranteed).toBe(mode === 'off');
      expect(onCaptureOff).toHaveBeenCalledTimes(mode === 'off' ? 1 : 0);
    },
  );

  it('follows a stale recording acknowledgement with cancel-only', async () => {
    const recording = deferred<unknown>();
    const setSessionCapture = vi
      .fn<(mode: HelperSessionCaptureMode) => Promise<unknown>>()
      .mockReturnValueOnce(recording.promise)
      .mockResolvedValue({ mode: 'cancel-only' });
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      vi.fn(),
    );
    const generation = reconciler.beginGeneration();

    const first = reconciler.request('recording', generation);
    await vi.waitFor(() => expect(setSessionCapture).toHaveBeenCalledOnce());
    const latest = reconciler.request('cancel-only', generation);
    recording.resolve({ mode: 'recording' });
    await Promise.all([first, latest]);

    expect(setSessionCapture.mock.calls.map(([mode]) => mode)).toEqual([
      'recording',
      'cancel-only',
    ]);
    expect(reconciler.captureOffGuaranteed).toBe(false);
  });

  it('rejects a helper acknowledgement for a different capture mode', async () => {
    const setSessionCapture = vi.fn(() => Promise.resolve({ mode: 'recording' }));
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      vi.fn(),
    );
    const generation = reconciler.beginGeneration();

    await expect(reconciler.request('cancel-only', generation)).rejects.toThrow(
      'wrong capture mode',
    );
    expect(reconciler.captureOffGuaranteed).toBe(false);
  });

  it('ignores requests from an obsolete capture generation', async () => {
    const setSessionCapture = vi.fn(() => Promise.resolve());
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture: () => Promise.resolve() },
      vi.fn(),
    );
    const obsolete = reconciler.beginGeneration();
    reconciler.beginGeneration();

    await reconciler.request('recording', obsolete);

    expect(setSessionCapture).not.toHaveBeenCalled();
  });

  it('retries capture-off after transient helper request capacity is released', async () => {
    const capacity = Object.assign(new Error('helper busy'), { code: 'request-capacity' });
    const setSessionCapture = vi
      .fn<(mode: HelperSessionCaptureMode) => Promise<unknown>>()
      .mockRejectedValueOnce(capacity)
      .mockResolvedValue({ mode: 'off' });
    const resetSessionCapture = vi.fn(() => Promise.resolve());
    const onCaptureOff = vi.fn();
    const reconciler = new HelperCaptureReconciler(
      { setSessionCapture, resetSessionCapture },
      onCaptureOff,
    );
    const generation = reconciler.beginGeneration();
    reconciler.markNativeCaptureArmed();

    await reconciler.request('off', generation);

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

      await expect(reconciler.request('off', generation)).rejects.toBe(error);
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
    const request = reconciler.request('off', generation);
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
    const request = reconciler.request('off', generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());

    reconciler.beginShutdown();

    await expect(request).rejects.toThrow('disable failed');
    expect(resetSignal?.aborted).toBe(true);
    expect(reconciler.captureOffGuaranteed).toBe(false);
  });

  it('observes best-effort disable failures and allows a later recovery', async () => {
    let failing = true;
    const setSessionCapture = vi.fn((mode: HelperSessionCaptureMode) =>
      mode === 'off' && failing
        ? Promise.reject(new Error('disable failed'))
        : Promise.resolve({ mode }),
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

    reconciler.requestBestEffort('off', generation);
    await vi.waitFor(() => expect(resetSessionCapture).toHaveBeenCalledOnce());
    expect(reconciler.captureOffGuaranteed).toBe(false);

    failing = false;
    reconciler.markAppliedUnknown();
    await reconciler.request('off', generation);

    expect(reconciler.captureOffGuaranteed).toBe(true);
    expect(onCaptureOff).toHaveBeenCalledOnce();
  });
});
