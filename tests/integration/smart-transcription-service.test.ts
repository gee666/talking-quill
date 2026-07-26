import { readdir } from 'node:fs/promises';
import { describe, expect, it, vi } from 'vitest';
import { ProviderError } from '../../app/src/main/providers/errors';
import { SmartTranscriptionService } from '../../app/src/main/smart/smart-transcription-service';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import type { ProviderService } from '../../app/src/main/providers/provider-service';
import type { ScreenshotService } from '../../app/src/main/screenshot/screenshot-service';
import type { ProviderCompletionRequest } from '../../app/src/shared/schemas/providers';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const VALID_JPEG_BASE64 =
  '/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAYEBQYFBAYGBQYHBwYIChAKCgkJChQODwwQFxQYGBcUFhYaHSUfGhsjHBYWICwgIyYnKSopGR8tMC0oMCUoKSj/2wBDAQcHBwoIChMKChMoGhYaKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCgoKCj/wAARCABAAEADASIAAhEBAxEB/8QAFQABAQAAAAAAAAAAAAAAAAAAAAf/xAAUEAEAAAAAAAAAAAAAAAAAAAAA/8QAFgEBAQEAAAAAAAAAAAAAAAAAAAYI/8QAFBEBAAAAAAAAAAAAAAAAAAAAAP/aAAwDAQACEQMRAD8AnQCOaRAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAf//Z';

function createHarness(
  options: {
    osa?: boolean;
    output?: string;
    completion?: () => Promise<string>;
    retain?: boolean;
    screenshotsDirectory?: string;
    manualVision?: boolean;
    providerManagedModel?: boolean;
    preflightCapability?: 'supported' | 'unsupported' | 'unknown';
    settingsUpdateGate?: Promise<void>;
    retainScreenshot?: () => {
      readonly filename: string;
      cleanup(): void;
    };
  } = {},
) {
  const providerId = options.providerManagedModel
    ? ('textgenwebui' as const)
    : options.manualVision
      ? ('generic-openai' as const)
      : ('openai' as const);
  const config = options.providerManagedModel
    ? {
        providerId,
        baseUrl: 'http://127.0.0.1:5000/v1',
        maxOutputTokens: 9_999,
      }
    : options.manualVision
      ? {
          providerId,
          baseUrl: 'http://127.0.0.1:8080/v1',
          modelId: 'private-model',
          maxOutputTokens: 9_999,
        }
      : { providerId, modelId: 'gpt-4.1', maxOutputTokens: 9_999 };
  let settings: Settings = {
    ...structuredClone(DEFAULT_SETTINGS),
    smartProcessing: {
      ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
      selectedProviderId: providerId,
      providers: {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing.providers),
        [providerId]: options.providerManagedModel
          ? {
              baseUrl: 'http://127.0.0.1:5000/v1',
              maxOutputTokens: 9_999,
            }
          : options.manualVision
            ? {
                baseUrl: 'http://127.0.0.1:8080/v1',
                modelId: 'private-model',
                maxOutputTokens: 9_999,
              }
            : { modelId: 'gpt-4.1', maxOutputTokens: 9_999 },
      },
      onScreenAwarenessEnabled: options.osa ?? false,
    },
    privacy: {
      ...structuredClone(DEFAULT_SETTINGS.privacy),
      retainSmartScreenshots: options.retain ?? false,
    },
  };
  let revision = 0;
  const revisionListeners = new Set<(revision: number) => void>();
  const settingsListeners = new Set<(settings: Settings) => void>();
  const update = vi.fn(
    async (
      patch: {
        smartProcessing?: Partial<Settings['smartProcessing']>;
        privacy?: Partial<Settings['privacy']>;
        customVocabulary?: Settings['customVocabulary'];
      },
      signal?: AbortSignal,
    ) => {
      const assertActive = (): void => {
        if (signal?.aborted === true) throw new DOMException('Cancelled', 'AbortError');
      };
      assertActive();
      await options.settingsUpdateGate;
      assertActive();
      const smartChanged =
        patch.smartProcessing !== undefined || patch.customVocabulary !== undefined;
      settings = {
        ...settings,
        smartProcessing: { ...settings.smartProcessing, ...patch.smartProcessing },
        privacy: { ...settings.privacy, ...patch.privacy },
        customVocabulary: patch.customVocabulary ?? settings.customVocabulary,
      };
      if (smartChanged) {
        revision += 1;
        for (const listener of revisionListeners) listener(revision);
      }
      for (const listener of settingsListeners) listener(settings);
      return settings;
    },
  );
  const capturedRequests: ProviderCompletionRequest[] = [];
  const cleanTranscript = vi.fn(
    (_config: unknown, request: ProviderCompletionRequest, signal: AbortSignal) => {
      if (signal.aborted) return Promise.reject(new DOMException('Aborted', 'AbortError'));
      capturedRequests.push(request);
      const result =
        options.completion?.() ?? Promise.resolve(options.output ?? '```text\nClean result\n```');
      return new Promise<string>((resolve, reject) => {
        const abort = () => reject(new ProviderError('CANCELLED'));
        signal.addEventListener('abort', abort, { once: true });
        void result.then(
          (value) => {
            signal.removeEventListener('abort', abort);
            resolve(value);
          },
          (error: unknown) => {
            signal.removeEventListener('abort', abort);
            reject(error instanceof Error ? error : new Error('Provider test failed'));
          },
        );
      });
    },
  );
  const capture = vi.fn(() =>
    Promise.resolve({
      image: { mimeType: 'image/jpeg' as const, base64: VALID_JPEG_BASE64 },
      width: 100,
      height: 50,
    }),
  );
  const retainScreenshot = options.retainScreenshot;
  const service = new SmartTranscriptionService({
    settings: {
      get: () => settings,
      update,
      subscribe: (listener: (next: Settings) => void) => {
        settingsListeners.add(listener);
        return () => settingsListeners.delete(listener);
      },
    } as unknown as SettingsStore,
    configs: {
      get: () => ({
        ...config,
        ...settings.smartProcessing.providers[providerId],
      }),
      smartRevision: () => revision,
      subscribeSmartRevision: (listener: (next: number) => void) => {
        revisionListeners.add(listener);
        return () => revisionListeners.delete(listener);
      },
    } as unknown as ProviderConfigService,
    providers: {
      credentialBinding: () => (options.manualVision ? 'generic-binding' : 'openai'),
      capabilities: () => (options.manualVision ? ('unknown' as const) : ('supported' as const)),
      preflightCapability: () =>
        Promise.resolve(
          options.preflightCapability ??
            (options.manualVision ? ('unknown' as const) : ('supported' as const)),
        ),
      cleanTranscript,
    } as unknown as ProviderService,
    screenshots: {
      permissionStatus: () => 'granted' as const,
      capture,
    } as unknown as ScreenshotService,
    helper: {
      getFrontApp: () =>
        Promise.resolve({
          processName: 'target',
          windowTitle: 'document',
          windowBounds: { x: 0, y: 0, width: 100, height: 100 },
        }),
    },
    screenshotsDirectory: options.screenshotsDirectory ?? 'unused',
    ...(retainScreenshot === undefined ? {} : { retainScreenshot: () => retainScreenshot() }),
  });
  const invalidateConfig = () => {
    revision += 1;
    for (const listener of revisionListeners) listener(revision);
  };
  const subscriptionCounts = () => ({
    revision: revisionListeners.size,
    settings: settingsListeners.size,
  });
  return {
    service,
    capturedRequests,
    cleanTranscript,
    capture,
    update,
    invalidateConfig,
    subscriptionCounts,
  };
}

describe('SmartTranscriptionService', () => {
  it('freezes config, normalizes output, and sends one bounded deterministic text request', async () => {
    const { service, capturedRequests, capture } = createHarness();
    const session = service.beginSession();
    const result = await session.process('raw words', new AbortController().signal);
    expect(result).toEqual({ text: 'Clean result', screenshotFilename: null });
    expect(capture).not.toHaveBeenCalled();
    expect(capturedRequests).toHaveLength(1);
    expect(capturedRequests[0]).toMatchObject({
      modelId: 'gpt-4.1',
      temperature: 0.2,
      maxOutputTokens: 9_999,
    });
    expect(capturedRequests[0]?.input).toContain('Untrusted transcript JSON:\n"raw words"');
  });

  it('processes provider-managed model sessions without inventing a model ID', async () => {
    const { service, capturedRequests } = createHarness({ providerManagedModel: true });
    const session = service.beginSession();

    expect(session).toMatchObject({ providerId: 'textgenwebui', modelId: null });
    await expect(session.process('raw words', new AbortController().signal)).resolves.toMatchObject(
      { text: 'Clean result' },
    );
    expect(capturedRequests).toHaveLength(1);
    expect(capturedRequests[0]).not.toHaveProperty('modelId');
  });

  it.each([
    ['empty', '   '],
    ['fence-only', '```text\n\n```'],
    ['oversized', 'x'.repeat(1_000_001)],
  ])('rejects %s provider output for controller fallback', async (_name, output) => {
    const { service } = createHarness({ output });
    await expect(
      service.beginSession().process('raw words', new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_RESPONSE' });
  });

  it('surfaces provider timeout and frozen-configuration failures for raw fallback', async () => {
    const timed = createHarness({
      completion: () => Promise.reject(new ProviderError('TIMEOUT')),
    });
    await expect(
      timed.service.beginSession().process('raw words', new AbortController().signal),
    ).rejects.toMatchObject({ code: 'TIMEOUT' });

    const stale = createHarness();
    const session = stale.service.beginSession();
    await stale.update({ smartProcessing: { selectedProviderId: 'ollama' } });
    await expect(session.process('raw words', new AbortController().signal)).rejects.toMatchObject({
      code: 'STALE_CONFIG',
    });
  });

  it('captures at most one image for the one provider request when OSA is enabled', async () => {
    const { service, capturedRequests, capture } = createHarness({ osa: true });
    const session = service.beginSession();
    const controller = new AbortController();
    await session.prepare(controller.signal);
    await session.process('raw words', controller.signal);
    expect(capture).toHaveBeenCalledTimes(1);
    expect(capturedRequests).toHaveLength(1);
    expect(capturedRequests[0]?.image).toEqual({
      mimeType: 'image/jpeg',
      base64: VALID_JPEG_BASE64,
    });
  });

  it('fails closed before capture when async capability preflight is explicitly non-vision', async () => {
    const test = createHarness({ osa: true, preflightCapability: 'unsupported' });
    const session = test.service.beginSession();
    const controller = new AbortController();
    await session.prepare(controller.signal);
    expect(test.capture).not.toHaveBeenCalled();
    await expect(session.process('raw', controller.signal)).rejects.toMatchObject({
      code: 'INVALID_CONFIG',
    });
    expect(test.cleanTranscript).not.toHaveBeenCalled();
  });

  it('does not create retained files when an abort-ignoring provider resolves after cleanup', async () => {
    const directory = await createTestDirectory('smart-abort-retention');
    const pendingProvider: { resolve: ((output: string) => void) | null } = { resolve: null };
    const providerResult = new Promise<string>((resolve) => {
      pendingProvider.resolve = resolve;
    });
    try {
      const { service, cleanTranscript } = createHarness({
        osa: true,
        retain: true,
        screenshotsDirectory: directory,
        completion: () => providerResult,
      });
      const controller = new AbortController();
      const session = service.beginSession();
      await session.prepare(controller.signal);
      const processing = session.process('raw', controller.signal);
      await vi.waitFor(() => expect(cleanTranscript).toHaveBeenCalledOnce());
      controller.abort();
      session.cleanup();
      if (pendingProvider.resolve === null) throw new Error('Provider operation was not pending');
      pendingProvider.resolve('late polished response');
      await expect(processing).rejects.toMatchObject({ code: 'CANCELLED' });
      expect(
        (await readdir(directory)).filter((entry) => !entry.startsWith('.talking-quill-')),
      ).toEqual([]);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('keeps the manual vision settings commit bound to the operation signal', async () => {
    const { service, update } = createHarness({ manualVision: true, output: 'ECHO-1234' });
    const controller = new AbortController();
    const verification = await service.verifyManualVision('ECHO-1234', controller.signal);
    expect(update).not.toHaveBeenCalled();
    await service.confirmManualVision(verification.verificationId, controller.signal);
    expect(update).toHaveBeenCalledOnce();
    expect(update.mock.calls[0]?.[0].smartProcessing?.visionOverrides).toHaveLength(1);
    expect(update.mock.calls[0]?.[1]).toBeInstanceOf(AbortSignal);
  });

  it('rejects OSA consent when configuration changes while its settings write is queued', async () => {
    const gate = deferred<undefined>();
    const test = createHarness({ settingsUpdateGate: gate.promise });
    const enabling = test.service.setOnScreenAwareness(true);
    await vi.waitFor(() => expect(test.update).toHaveBeenCalledOnce());
    test.invalidateConfig();
    gate.resolve(undefined);
    await expect(enabling).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    expect(test.subscriptionCounts()).toEqual({ revision: 0, settings: 0 });
  });

  it('rejects manual vision consent when configuration changes during its settings write', async () => {
    const gate = deferred<undefined>();
    const test = createHarness({
      manualVision: true,
      output: 'ECHO-1234',
      settingsUpdateGate: gate.promise,
    });
    const verification = await test.service.verifyManualVision(
      'ECHO-1234',
      new AbortController().signal,
    );
    const confirmation = test.service.confirmManualVision(
      verification.verificationId,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(test.update).toHaveBeenCalledOnce());
    test.invalidateConfig();
    gate.resolve(undefined);
    await expect(confirmation).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    expect(test.subscriptionCounts()).toEqual({ revision: 0, settings: 0 });
  });

  it('does not save a late manual vision override after visible cancellation', async () => {
    const pendingProvider: { resolve: ((output: string) => void) | null } = { resolve: null };
    const providerResult = new Promise<string>((resolve) => {
      pendingProvider.resolve = resolve;
    });
    const { service, cleanTranscript, update } = createHarness({
      manualVision: true,
      completion: () => providerResult,
    });
    const controller = new AbortController();
    const verification = service.verifyManualVision('ECHO-1234', controller.signal);
    await vi.waitFor(() => expect(cleanTranscript).toHaveBeenCalledOnce());
    controller.abort();
    if (pendingProvider.resolve === null)
      throw new Error('Vision provider operation was not pending');
    pendingProvider.resolve('ECHO-1234');
    await expect(verification).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(update).not.toHaveBeenCalled();
  });

  it.each([
    ['provider selection', { smartProcessing: { selectedProviderId: 'ollama' as const } }],
    [
      'provider model and limits',
      { smartProcessing: { providers: { openai: { modelId: 'gpt-5', maxOutputTokens: 512 } } } },
    ],
    ['credential epoch', { smartProcessing: { credentialEpochs: { openai: 1 } } }],
    ['OSA consent', { smartProcessing: { onScreenAwarenessEnabled: true } }],
    [
      'manual vision consent',
      {
        smartProcessing: {
          visionOverrides: [
            {
              providerId: 'generic-openai' as const,
              binding: 'binding',
              modelId: 'model',
              verifiedAt: 1,
            },
          ],
        },
      },
    ],
    [
      'vocabulary',
      { customVocabulary: [{ id: '11111111-1111-4111-8111-111111111111', value: 'Acme' }] },
    ],
  ])('aborts active Smart work on monotonic %s revision', async (_name, patch) => {
    const pending = deferred<string>();
    const test = createHarness({ completion: () => pending.promise });
    const session = test.service.beginSession();
    const processing = session.process('raw words', new AbortController().signal);
    await vi.waitFor(() => expect(test.cleanTranscript).toHaveBeenCalledOnce());
    await test.update(patch as Parameters<typeof test.update>[0]);
    await expect(processing).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    pending.resolve('late output');
  });

  it('makes OSA revocation in flight irreversible even when consent is enabled again', async () => {
    const pending = deferred<string>();
    const test = createHarness({ osa: true, completion: () => pending.promise });
    const session = test.service.beginSession();
    const controller = new AbortController();
    await session.prepare(controller.signal);
    const processing = session.process('raw', controller.signal);
    await vi.waitFor(() => expect(test.cleanTranscript).toHaveBeenCalledOnce());
    await test.update({ smartProcessing: { onScreenAwarenessEnabled: false } });
    await test.update({ smartProcessing: { onScreenAwarenessEnabled: true } });
    await expect(processing).rejects.toMatchObject({ code: 'STALE_CONFIG' });
    pending.resolve('late output');
  });

  it('keeps valid Smart output when optional screenshot retention cannot be created', async () => {
    const test = createHarness({
      osa: true,
      retain: true,
      retainScreenshot: () => {
        throw new Error('retention unavailable');
      },
    });
    const session = test.service.beginSession();
    const signal = new AbortController().signal;
    await session.prepare(signal);
    await expect(session.process('raw', signal)).resolves.toEqual({
      text: 'Clean result',
      screenshotFilename: null,
    });
    session.cleanup();
    expect(test.subscriptionCounts()).toEqual({ revision: 0, settings: 0 });
  });

  it('always disposes session subscriptions when retained screenshot cleanup throws', async () => {
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
    const test = createHarness({
      osa: true,
      retain: true,
      retainScreenshot: () => ({ filename: 'pending.jpg', cleanup }),
    });
    const session = test.service.beginSession();
    const signal = new AbortController().signal;
    await session.prepare(signal);
    await expect(session.process('raw', signal)).resolves.toMatchObject({
      screenshotFilename: 'pending.jpg',
    });
    expect(() => session.cleanup()).toThrow('cleanup failed');
    expect(cleanup).toHaveBeenCalledOnce();
    expect(test.subscriptionCounts()).toEqual({ revision: 0, settings: 0 });
  });

  it('maps internal session disposal to cancellation for an active provider operation', async () => {
    const pending = deferred<string>();
    const test = createHarness({ completion: () => pending.promise });
    const session = test.service.beginSession();
    const processing = session.process('raw', new AbortController().signal);
    await vi.waitFor(() => expect(test.cleanTranscript).toHaveBeenCalledOnce());
    session.cleanup();
    await expect(processing).rejects.toMatchObject({ code: 'CANCELLED' });
    pending.resolve('late output');
  });

  it('applies privacy revocation in flight without accepting a later retention grant', async () => {
    const directory = await createTestDirectory('smart-one-way-privacy');
    const pending = deferred<string>();
    try {
      const test = createHarness({
        osa: true,
        retain: true,
        screenshotsDirectory: directory,
        completion: () => pending.promise,
      });
      const session = test.service.beginSession();
      const controller = new AbortController();
      await session.prepare(controller.signal);
      const processing = session.process('raw', controller.signal);
      await vi.waitFor(() => expect(test.cleanTranscript).toHaveBeenCalledOnce());
      await test.update({ privacy: { retainSmartScreenshots: false } });
      await test.update({ privacy: { retainSmartScreenshots: true } });
      pending.resolve('clean output');
      await expect(processing).resolves.toEqual({ text: 'clean output', screenshotFilename: null });
      expect(
        (await readdir(directory)).filter((entry) => !entry.startsWith('.talking-quill-')),
      ).toEqual([]);
    } finally {
      await removeTestDirectory(directory);
    }
  });

  it('does not issue a provider request after cancellation', async () => {
    const { service, cleanTranscript, capture } = createHarness({ osa: true });
    const controller = new AbortController();
    controller.abort();
    await expect(service.beginSession().prepare(controller.signal)).rejects.toBeDefined();
    expect(capture).not.toHaveBeenCalled();
    expect(cleanTranscript).not.toHaveBeenCalled();
  });
});

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}
