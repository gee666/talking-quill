import { describe, expect, it, vi } from 'vitest';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import type { ProviderService } from '../../app/src/main/providers/provider-service';
import type { PreparedCompletionLease } from '../../app/src/main/providers/contracts';
import { ProviderError } from '../../app/src/main/providers/errors';
import { SmartTranscriptionService } from '../../app/src/main/smart/smart-transcription-service';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';

function deferred<T>() {
  let resolve: (value: T) => void = () => undefined;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function setup(prepareCompletion: () => Promise<PreparedCompletionLease>) {
  const settings = structuredClone(DEFAULT_SETTINGS);
  settings.smartProcessing.selectedProviderId = 'pi';
  settings.smartProcessing.onScreenAwarenessEnabled = false;
  let revision = 0;
  const listeners = new Set<(revision: number) => void>();
  const removePrivacy = vi.fn();
  const prepare = vi.fn(prepareCompletion);
  const cleanTranscript = vi.fn(() => Promise.resolve('ordinary fallback'));
  const capture = vi.fn();
  const service = new SmartTranscriptionService({
    settings: { get: () => settings, subscribe: () => removePrivacy } as unknown as SettingsStore,
    configs: {
      get: () => ({ providerId: 'pi', modelId: 'test/model', thinking: 'off' }),
      smartRevision: () => revision,
      subscribeSmartRevision: (listener: (value: number) => void) => {
        listeners.add(listener);
        return () => listeners.delete(listener);
      },
    } as unknown as ProviderConfigService,
    providers: {
      credentialBinding: () => 'binding',
      prepareCompletion: prepare,
      cleanTranscript,
    } as unknown as ProviderService,
    screenshots: { capture, permissionStatus: () => 'granted' },
    helper: { getFrontApp: vi.fn() },
    screenshotsDirectory: 'unused',
  });
  return {
    session: service.beginSession(),
    prepare,
    cleanTranscript,
    capture,
    removePrivacy,
    listeners,
    invalidate: () => {
      revision += 1;
      for (const listener of listeners) listener(revision);
    },
  };
}

function lease(complete = vi.fn(() => Promise.resolve('prepared output'))) {
  return {
    complete,
    requestClose: vi.fn(),
    closed: Promise.resolve(),
  } satisfies PreparedCompletionLease;
}

describe('extracted smart session lifecycle', () => {
  it('joins listening and submission preparation without capturing or duplicating a lease', async () => {
    const pending = deferred<PreparedCompletionLease>();
    const prepared = lease();
    const value = setup(() => pending.promise);
    const signal = new AbortController().signal;
    try {
      const listening = value.session.prepareForListening?.(signal);
      expect(value.session.prepareForListening?.(signal)).toBe(listening);
      const preparation = value.session.prepare(signal);
      expect(value.session.prepare(signal)).toBe(preparation);
      const processing = value.session.process('raw text', signal);
      expect(value.prepare).toHaveBeenCalledOnce();
      expect(prepared.complete).not.toHaveBeenCalled();
      expect(value.capture).not.toHaveBeenCalled();
      pending.resolve(prepared);
      await Promise.all([listening, preparation]);
      await expect(processing).resolves.toEqual({
        text: 'prepared output',
        screenshotFilename: null,
      });
      expect(prepared.complete).toHaveBeenCalledOnce();
      expect(value.cleanTranscript).not.toHaveBeenCalled();
      await expect(value.session.process('again', signal)).rejects.toMatchObject({
        code: 'INVALID_CONFIG',
      });
    } finally {
      pending.resolve(prepared);
      value.session.cleanup();
    }
  });

  it('closes a lease arriving after cleanup and removes subscriptions synchronously', async () => {
    const pending = deferred<PreparedCompletionLease>();
    const prepared = lease();
    const value = setup(() => pending.promise);
    const preparation = value.session.prepareForListening?.(new AbortController().signal);
    const rejected = expect(preparation).rejects.toMatchObject({ code: 'CANCELLED' });
    value.session.cleanup();
    expect(value.listeners.size).toBe(0);
    expect(value.removePrivacy).toHaveBeenCalledOnce();
    pending.resolve(prepared);
    await rejected;
    expect(prepared.requestClose).toHaveBeenCalledExactlyOnceWith('cancelled');
    expect(prepared.complete).not.toHaveBeenCalled();
    value.session.cleanup();
    expect(value.removePrivacy).toHaveBeenCalledOnce();
  });

  it('closes a consuming lease on revision change and never falls back after invalidation', async () => {
    const started = deferred<undefined>();
    const completion = deferred<string>();
    const prepared = lease(
      vi.fn(() => {
        started.resolve(undefined);
        return completion.promise;
      }),
    );
    const value = setup(() => Promise.resolve(prepared));
    try {
      const processing = value.session.process('raw', new AbortController().signal);
      const rejected = expect(processing).rejects.toMatchObject({ code: 'STALE_CONFIG' });
      await started.promise;
      value.invalidate();
      expect(prepared.requestClose).toHaveBeenCalledExactlyOnceWith('stale-config');
      completion.resolve('late output');
      await rejected;
      expect(value.cleanTranscript).not.toHaveBeenCalled();
    } finally {
      completion.resolve('late output');
      value.session.cleanup();
    }
  });

  it('keeps caller cancellation ahead of stale configuration when a prepared lease arrives', async () => {
    const pending = deferred<PreparedCompletionLease>();
    const prepared = lease();
    const value = setup(() => pending.promise);
    const caller = new AbortController();
    try {
      const preparation = value.session.prepareForListening?.(caller.signal);
      const rejected = expect(preparation).rejects.toMatchObject({ code: 'CANCELLED' });
      value.invalidate();
      caller.abort(new ProviderError('CANCELLED'));
      pending.resolve(prepared);
      await rejected;
      expect(prepared.requestClose).toHaveBeenCalledExactlyOnceWith('stale-config');
    } finally {
      pending.resolve(prepared);
      value.session.cleanup();
    }
  });
});
