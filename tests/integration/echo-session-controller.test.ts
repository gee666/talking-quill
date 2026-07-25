import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  discardChunkPrefix,
  EchoSessionController,
  type EchoHelperPort,
  type EchoHistoryPort,
  type EchoModelUseGrant,
  type SmartTranscriptProcessor,
  type VoiceCommandMatcherPort,
} from '../../app/src/main/echo/echo-session-controller';
import type {
  RecordingService,
  DictationCaptureCallbacks,
} from '../../app/src/main/audio/recording-service';
import type { HelperClient } from '../../app/src/main/helper';
import type { InsertionService } from '../../app/src/main/insertion/insertion-service';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { WhisperWorkerClient } from '../../app/src/main/transcription/whisper-worker-client';
import type { WindowManager } from '../../app/src/main/app/window-manager';
import { ProviderError } from '../../app/src/main/providers/errors';
import type {
  JsonTransport,
  JsonTransportRequest,
} from '../../app/src/main/providers/json-transport';
import { ProviderRegistry } from '../../app/src/main/providers/registry';
import { ProviderService } from '../../app/src/main/providers/provider-service';
import type { ProviderConfigService } from '../../app/src/main/providers/provider-config-service';
import type { ScreenshotService } from '../../app/src/main/screenshot/screenshot-service';
import { SmartTranscriptionService } from '../../app/src/main/smart/smart-transcription-service';
import type { HelperNotification } from '../../app/src/shared/helper/protocol';
import type { HelperReadiness } from '../../app/src/shared/schemas/helper-readiness';
import {
  DEFAULT_SETTINGS,
  SettingsSchema,
  type Settings,
  type SettingsPatch,
} from '../../app/src/shared/schemas/settings';

function fixture(
  options: {
    readonly insert?: InsertionService['insert'];
    readonly helperReadiness?: HelperReadiness;
    readonly startDictation?: RecordingService['startDictation'];
    readonly smartProcessor?: SmartTranscriptProcessor;
    readonly commands?: VoiceCommandMatcherPort;
    readonly isModelReady?: () => boolean;
    readonly setSessionCapture?: HelperClient['setSessionCapture'];
    readonly resetSessionCapture?: HelperClient['resetSessionCapture'];
    readonly transcribe?: WhisperWorkerClient['transcribe'];
    readonly startSession?: WhisperWorkerClient['startSession'];
    readonly acquireModelUse?: (
      modelId: Settings['transcription']['modelId'],
      signal: AbortSignal,
    ) => Promise<EchoModelUseGrant>;
    readonly configureActivation?: EchoHelperPort['configureActivation'];
    readonly historyRecord?: EchoHistoryPort['record'];
  } = {},
) {
  let notification: ((value: HelperNotification) => void) | null = null;
  let readinessListener: ((value: HelperReadiness) => void) | null = null;
  let captureCallbacks: DictationCaptureCallbacks | null = null;
  let current = structuredClone(DEFAULT_SETTINGS);
  const settingsListeners = new Set<(settings: Settings) => void>();
  const settings = {
    get: () => structuredClone(current),
    update: (patch: SettingsPatch) => {
      const dictationProfiles = patch.dictationProfiles ?? current.dictationProfiles;
      const general = dictationProfiles.find((profile) => profile.id === 'general');
      if (general === undefined) throw new Error('General profile is missing');
      current = SettingsSchema.parse({
        ...current,
        app: {
          ...current.app,
          ...defined(patch.app),
          activationKey: general.activationKey,
          defaultProcessingMode: general.processingMode,
        },
        recording: { ...current.recording, ...defined(patch.recording) },
        transcription: { ...current.transcription, ...defined(patch.transcription) },
        dictationProfiles,
      });
      for (const listener of settingsListeners) listener(structuredClone(current));
      return Promise.resolve(structuredClone(current));
    },
    subscribe: (listener: (settings: Settings) => void) => {
      settingsListeners.add(listener);
      return () => settingsListeners.delete(listener);
    },
  } as unknown as SettingsStore;
  const configureActivation = vi.fn(
    options.configureActivation ??
      ((enabled: boolean, bindings: Parameters<EchoHelperPort['configureActivation']>[1]) =>
        Promise.resolve({ enabled, bindings })),
  );
  const setSessionCapture = vi.fn(
    options.setSessionCapture ?? ((active: boolean) => Promise.resolve({ active })),
  );
  const resetSessionCapture = vi.fn(options.resetSessionCapture ?? (() => Promise.resolve()));
  const helper = {
    readiness:
      options.helperReadiness ??
      ({
        status: 'ready',
        reason: null,
        helperVersion: '1.0.0',
        permissions: {
          accessibility: 'not_applicable',
          inputMonitoring: 'not_applicable',
          eventPost: 'not_applicable',
        },
      } satisfies HelperReadiness),
    subscribeNotifications: (listener: (value: HelperNotification) => void) => {
      notification = listener;
      return () => {
        notification = null;
      };
    },
    subscribeReadiness: (listener: (value: HelperReadiness) => void) => {
      readinessListener = listener;
      return () => {
        readinessListener = null;
      };
    },
    configureActivation,
    setSessionCapture,
    resetSessionCapture,
    getFrontApp: () =>
      Promise.resolve({
        processName: 'target',
        windowTitle: 'Target',
        windowBounds: { x: 100, y: 200, width: 800, height: 600 },
      }),
  } as unknown as HelperClient;
  const startDictation = vi.fn(
    options.startDictation ??
      ((callbacks: DictationCaptureCallbacks) => {
        captureCallbacks = callbacks;
        return Promise.resolve({
          captureId: '00000000-0000-4000-8000-000000000010',
          activeMicrophoneId: 'default',
        });
      }),
  );
  const stopDictation = vi.fn(() => Promise.resolve());
  const recording = { startDictation, stopDictation } as unknown as RecordingService;
  const transcribe = vi.fn(
    options.transcribe ??
      (() =>
        Promise.resolve({
          text: 'locally transcribed',
          modelId: 'Xenova/whisper-small' as const,
          durationMs: 5,
          pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
        })),
  );
  const finish = vi.fn(() =>
    Promise.resolve({
      text: 'extended transcription',
      modelId: 'Xenova/whisper-small' as const,
      durationMs: 5,
      pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
    }),
  );
  const cancelStream = vi.fn(() => Promise.resolve());
  const startSession = vi.fn(
    options.startSession ??
      (() =>
        Promise.resolve({
          id: 'stream',
          push: () => Promise.resolve(),
          finish,
          cancel: cancelStream,
        })),
  );
  const whisper = {
    startSession,
    transcribe,
  } as unknown as WhisperWorkerClient;
  const insert = vi.fn(
    options.insert ?? (() => Promise.resolve({ inserted: true, copied: false })),
  );
  const insertion = { insert } as unknown as InsertionService;
  const showWidget = vi.fn();
  const windows = {
    showWidget,
    hideWidget: vi.fn(),
  } as unknown as WindowManager;
  const events = { send: vi.fn() } as unknown as IpcEventEmitter;
  const sound = vi.fn();
  const historyRecord = vi.fn(options.historyRecord ?? (() => true));
  const controller = new EchoSessionController({
    settings,
    recording,
    whisper,
    helper,
    insertion,
    history: { record: historyRecord },
    windows,
    events,
    ...(options.smartProcessor === undefined ? {} : { smartProcessor: options.smartProcessor }),
    ...(options.commands === undefined ? {} : { commands: options.commands }),
    ...(options.isModelReady === undefined ? {} : { isModelReady: options.isModelReady }),
    ...(options.acquireModelUse === undefined ? {} : { acquireModelUse: options.acquireModelUse }),
    sound,
  });
  controller.initialize();
  return {
    controller,
    helper,
    recording,
    whisper,
    insertion,
    windows,
    spies: {
      transcribe,
      insert,
      startDictation,
      stopDictation,
      configureActivation,
      setSessionCapture,
      resetSessionCapture,
      startSession,
      finish,
      cancelStream,
      showWidget,
      sound,
      historyRecord,
    },
    notify(value: HelperNotification) {
      notification?.(value);
    },
    frame(samples = new Float32Array(320).fill(0.2), rms = 0.2) {
      captureCallbacks?.onFrame(samples, rms);
    },
    loseCapture() {
      captureCallbacks?.onUnexpectedStop('device-unavailable');
    },
    setHelperReadiness(value: HelperReadiness) {
      readinessListener?.(value);
    },
    setPrivacy(patch: Partial<Settings['privacy']>) {
      current = SettingsSchema.parse({
        ...current,
        privacy: { ...current.privacy, ...patch },
      });
      for (const listener of settingsListeners) listener(structuredClone(current));
    },
  };
}

function activation(
  phase: 'down' | 'up',
  shift = false,
  keyValue: Settings['dictationProfiles'][number]['activationKey'] = 'Z',
): HelperNotification {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase, key: keyValue, shift },
  };
}
function key(keyValue: 'escape' | 'enter'): HelperNotification {
  return { jsonrpc: '2.0', method: 'session.key', params: { key: keyValue, phase: 'down' } };
}
function defined<T extends object>(value: T | undefined): Partial<T> {
  if (value === undefined) return {};
  return Object.fromEntries(
    Object.entries(value).filter((entry) => entry[1] !== undefined),
  ) as Partial<T>;
}

async function settle(): Promise<void> {
  await vi.waitFor(() => Promise.resolve());
  await new Promise((resolve) => setTimeout(resolve, 0));
}

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<Value>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise;
    reject = rejectPromise;
  });
  return { promise, resolve, reject };
}

afterEach(() => vi.useRealTimers());

describe('EchoSessionController integration', () => {
  it('discards acknowledged PCM across every chunk boundary without aliasing retained samples', () => {
    const first = new Float32Array([1, 2, 3]);
    const second = new Float32Array([4, 5]);
    const third = new Float32Array([6, 7, 8]);
    const retained = discardChunkPrefix([first, second, third], 4);
    expect(retained.map((chunk) => [...chunk])).toEqual([[5], [6, 7, 8]]);
    second[1] = 99;
    expect([...(retained[0] ?? new Float32Array())]).toEqual([5]);
    expect(() => discardChunkPrefix([first], 4)).toThrow('unavailable PCM');
  });

  it('keeps legacy mirrors atomic and always resets each built-in to its reserved exact default', async () => {
    const test = fixture();
    let settings = await test.controller.updateProfile('general', {
      activationKey: 'G',
      shift: true,
      processingMode: 'smart',
    });
    expect(settings.app).toMatchObject({ activationKey: 'G', defaultProcessingMode: 'smart' });
    settings = await test.controller.updateProfile('prompt', {
      activationKey: 'P',
      shift: false,
      processingMode: 'raw',
    });
    expect(settings.app).toMatchObject({ activationKey: 'G', defaultProcessingMode: 'smart' });

    await expect(
      test.controller.createProfile({
        name: 'Reserved thief',
        activationKey: 'Z',
        shift: false,
        processingMode: 'raw',
        smartPrompt: null,
      }),
    ).rejects.toThrow(/reserved/i);

    settings = await test.controller.resetProfile('general');
    expect(settings.dictationProfiles.find((profile) => profile.id === 'general')).toEqual({
      id: 'general',
      name: 'General',
      activationKey: 'Z',
      shift: false,
      processingMode: 'raw',
      smartPrompt: null,
    });
    expect(settings.app).toMatchObject({ activationKey: 'Z', defaultProcessingMode: 'raw' });
    settings = await test.controller.resetProfile('prompt');
    expect(settings.dictationProfiles.find((profile) => profile.id === 'prompt')).toEqual({
      id: 'prompt',
      name: 'Prompt',
      activationKey: 'Z',
      shift: true,
      processingMode: 'smart',
      smartPrompt:
        'Make dictated prompts focused, concise, and clear. Remove duplication and make them as short as possible while retaining dense information and a human-readable structure. Use lists, tables, and other formatting when useful.',
    });
    expect(settings.app).toMatchObject({ activationKey: 'Z', defaultProcessingMode: 'raw' });
  });

  it('serializes complete profile CRUD transactions without losing concurrent updates', async () => {
    const firstConfiguration = deferred<{
      enabled: boolean;
      bindings: readonly {
        key: Settings['dictationProfiles'][number]['activationKey'];
        shift: boolean;
      }[];
    }>();
    const test = fixture();
    await settle();
    test.spies.configureActivation.mockClear();
    test.spies.configureActivation
      .mockImplementationOnce(() => firstConfiguration.promise)
      .mockImplementation((enabled, bindings) => Promise.resolve({ enabled, bindings }));

    const first = test.controller.createProfile({
      name: 'First concurrent profile',
      activationKey: 'Q',
      shift: false,
      processingMode: 'raw',
      smartPrompt: null,
    });
    const second = test.controller.createProfile({
      name: 'Second concurrent profile',
      activationKey: 'R',
      shift: false,
      processingMode: 'smart',
      smartPrompt: 'Second prompt.',
    });

    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledTimes(1));
    firstConfiguration.resolve({
      enabled: true,
      bindings: [
        { key: 'Z', shift: false },
        { key: 'Z', shift: true },
        { key: 'Q', shift: false },
      ],
    });
    await first;
    const authoritative = await second;

    expect(test.spies.configureActivation.mock.calls.length).toBeGreaterThanOrEqual(2);
    expect(authoritative.dictationProfiles.map((profile) => profile.name)).toEqual(
      expect.arrayContaining(['First concurrent profile', 'Second concurrent profile']),
    );
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      true,
      expect.arrayContaining([
        { key: 'Q', shift: false },
        { key: 'R', shift: false },
      ]),
    );
  });

  it('immutably snapshots the Prompt Smart preference across profile mutation, reset, and other deletion', async () => {
    const promptsUsed: (string | null | undefined)[] = [];
    const process = vi.fn((text: string, signal: AbortSignal) => {
      void text;
      void signal;
      return Promise.resolve({ text: 'cleaned prompt', screenshotFilename: null });
    });
    const beginSession = vi.fn<
      Extract<SmartTranscriptProcessor, { beginSession: unknown }>['beginSession']
    >((profile) => {
      const capturedPrompt = profile?.smartPrompt;
      return {
        providerId: 'openai',
        modelId: 'gpt-4.1',
        prepare: () => Promise.resolve(),
        process: (text, signal) => {
          promptsUsed.push(capturedPrompt);
          return process(text, signal);
        },
        commitScreenshot: vi.fn(),
        cleanup: vi.fn(),
      };
    });
    const test = fixture({ smartProcessor: { beginSession } });
    await test.controller.updateProfile('prompt', { smartPrompt: 'Original Prompt preference.' });
    const withCustom = await test.controller.createProfile({
      name: 'Temporary',
      activationKey: 'Q',
      shift: false,
      processingMode: 'raw',
      smartPrompt: null,
    });
    const custom = withCustom.dictationProfiles.find((profile) => profile.name === 'Temporary');
    expect(custom).toBeDefined();
    if (custom === undefined) throw new Error('Temporary profile was not created');

    test.notify(activation('down', true));
    await settle();
    expect(test.controller.snapshot.processingMode).toBe('smart');
    expect(test.controller.snapshot.alternate).toBe(true);
    await test.controller.updateProfile('prompt', { smartPrompt: 'Mutated preference.' });
    await test.controller.resetProfile('prompt');
    await test.controller.deleteProfile(custom.id);

    test.frame();
    test.notify(activation('up', true));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    expect(beginSession).toHaveBeenCalledOnce();
    expect(beginSession).toHaveBeenCalledWith(
      expect.objectContaining({
        id: 'prompt',
        shift: true,
        processingMode: 'smart',
        smartPrompt: 'Original Prompt preference.',
      }),
    );
    expect(promptsUsed).toEqual(['Original Prompt preference.']);
    expect(process).toHaveBeenCalledWith('locally transcribed', expect.any(AbortSignal));
  });

  it('keeps a selected custom Smart profile immutable when that profile is deleted', async () => {
    const promptsUsed: (string | null | undefined)[] = [];
    const beginSession = vi.fn<
      Extract<SmartTranscriptProcessor, { beginSession: unknown }>['beginSession']
    >((profile) => {
      const capturedPrompt = profile?.smartPrompt;
      return {
        providerId: 'openai',
        modelId: 'gpt-4.1',
        prepare: () => Promise.resolve(),
        process: () => {
          promptsUsed.push(capturedPrompt);
          return Promise.resolve({ text: 'custom cleaned', screenshotFilename: null });
        },
        commitScreenshot: vi.fn(),
        cleanup: vi.fn(),
      };
    });
    const test = fixture({ smartProcessor: { beginSession } });
    const settings = await test.controller.createProfile({
      name: 'Custom Smart',
      activationKey: 'Q',
      shift: true,
      processingMode: 'smart',
      smartPrompt: 'Deleted profile preference.',
    });
    const custom = settings.dictationProfiles.find((profile) => profile.name === 'Custom Smart');
    expect(custom).toBeDefined();
    if (custom === undefined) throw new Error('Custom Smart profile was not created');

    test.notify(activation('down', true, 'Q'));
    await settle();
    await test.controller.deleteProfile(custom.id);
    test.frame();
    test.notify(activation('up', true, 'Q'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    expect(beginSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: custom.id, smartPrompt: 'Deleted profile preference.' }),
    );
    expect(promptsUsed).toEqual(['Deleted profile preference.']);
  });

  it('selects exact key-and-Shift profiles while General remains raw', async () => {
    const beginSession = vi.fn();
    const test = fixture({
      smartProcessor: { beginSession } as unknown as SmartTranscriptProcessor,
    });

    test.notify(activation('down', false, 'Q'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('idle');

    test.notify(activation('down', false, 'Z'));
    await settle();
    expect(test.controller.snapshot.processingMode).toBe('raw');
    expect(test.controller.snapshot.alternate).toBe(false);
    test.frame();
    test.notify(activation('up', false, 'Z'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(beginSession).not.toHaveBeenCalled();
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ processingMode: 'raw', rawText: 'locally transcribed' }),
    );
  });

  it('inserts a matched command, bypasses Smart, and writes typed history after insertion', async () => {
    const smart = { process: vi.fn(() => Promise.resolve('must not run')) };
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'locally transcribed',
      snippet: 'inserted snippet',
      createdAt: 1,
      updatedAt: 1,
    };
    const test = fixture({
      smartProcessor: smart,
      commands: { match: () => ({ command, kind: 'exact', score: 1 }) },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(smart.process).not.toHaveBeenCalled();
    expect(test.spies.insert).toHaveBeenCalledWith(
      'inserted snippet',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
    expect(test.spies.historyRecord).toHaveBeenCalledWith({
      kind: 'voice-command',
      dictationMode: 'quick',
      processingMode: 'smart',
      rawText: 'locally transcribed',
      voiceTrigger: 'locally transcribed',
      voiceSnippet: 'inserted snippet',
    });
  });

  it('discards prepared Smart context when transcription resolves to a voice command', async () => {
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'locally transcribed',
      snippet: 'inserted snippet',
      createdAt: 1,
      updatedAt: 1,
    };
    const prepare = vi.fn(() => Promise.resolve());
    const process = vi.fn(() => Promise.resolve({ text: 'unused', screenshotFilename: null }));
    const cleanup = vi.fn();
    const test = fixture({
      commands: { match: () => ({ command, kind: 'exact', score: 1 }) },
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare,
          process,
          commitScreenshot: vi.fn(),
          cleanup,
        }),
      },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(prepare).toHaveBeenCalledOnce();
    expect(cleanup).toHaveBeenCalledOnce();
    expect(process).not.toHaveBeenCalled();
  });

  it('cancels submit-time Smart preparation and discards its session context', async () => {
    const cleanup = vi.fn();
    const prepare = vi.fn(
      (signal: AbortSignal) =>
        new Promise<void>((_resolve, reject) => {
          signal.addEventListener(
            'abort',
            () => reject(new DOMException('cancelled', 'AbortError')),
            { once: true },
          );
        }),
    );
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare,
          process: vi.fn(() => Promise.resolve({ text: 'unused', screenshotFilename: null })),
          commitScreenshot: vi.fn(),
          cleanup,
        }),
      },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(prepare).toHaveBeenCalledOnce());
    test.controller.cancel();
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('cancelled'));
    await vi.waitFor(() => expect(cleanup).toHaveBeenCalledOnce());
    expect(test.spies.transcribe).not.toHaveBeenCalled();
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
  });

  it('records no command history when cancellation wins before insertion commit', async () => {
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'locally transcribed',
      snippet: 'inserted snippet',
      createdAt: 1,
      updatedAt: 1,
    };
    const test = fixture({
      commands: { match: () => ({ command, kind: 'exact', score: 1 }) },
      insert: (_text, signal) =>
        new Promise((_resolve, reject) => {
          signal?.addEventListener(
            'abort',
            () => {
              const error = new Error('cancelled');
              error.name = 'AbortError';
              reject(error);
            },
            { once: true },
          );
        }),
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('inserting'));

    test.controller.cancel();

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('cancelled'));
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
    await test.controller.shutdown();
  });

  it('keeps one command outcome authoritative when shutdown follows insertion commit', async () => {
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'locally transcribed',
      snippet: 'inserted snippet',
      createdAt: 1,
      updatedAt: 1,
    };
    const restoration = deferred<undefined>();
    const test = fixture({
      commands: { match: () => ({ command, kind: 'exact', score: 1 }) },
      insert: async (_text, _signal, onCommitted) => {
        onCommitted?.();
        await restoration.promise;
        return { inserted: true, copied: false };
      },
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('restoringClipboard'));

    const shutdown = test.controller.shutdown();
    restoration.resolve(undefined);
    await shutdown;

    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'voice-command', voiceSnippet: 'inserted snippet' }),
    );
  });

  it('does not erase or misclassify committed command history when restoration fails', async () => {
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'locally transcribed',
      snippet: 'inserted snippet',
      createdAt: 1,
      updatedAt: 1,
    };
    const test = fixture({
      commands: { match: () => ({ command, kind: 'exact', score: 1 }) },
      insert: (_text, _signal, onCommitted) => {
        onCommitted?.();
        return Promise.reject(new Error('clipboard restoration failed'));
      },
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.completion).toBe('inserted');
    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'voice-command', voiceSnippet: 'inserted snippet' }),
    );
    await test.controller.shutdown();
  });

  it('fails closed when Whisper returns an oversized multibyte transcript', async () => {
    const test = fixture({
      transcribe: () =>
        Promise.resolve({
          text: 'é'.repeat(500_001),
          modelId: 'Xenova/whisper-small',
          durationMs: 5,
          pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
        }),
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
    expect(test.controller.snapshot.transcript).toBeNull();
    await test.controller.shutdown();
  });

  it('runs shortcut down → Quick → Enter → transcribe → insert → teardown', async () => {
    const test = fixture();
    test.notify(activation('down'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('arming');
    test.frame();
    test.notify(activation('up'));
    expect(test.controller.snapshot.phase).toBe('recordingQuick');
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
    expect(test.spies.insert).toHaveBeenCalledWith(
      'locally transcribed',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false);
    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
    expect(test.spies.historyRecord).toHaveBeenCalledWith({
      kind: 'raw-completed',
      dictationMode: 'quick',
      processingMode: 'raw',
      rawText: 'locally transcribed',
    });
  });

  it.each([
    { name: 'Raw', mode: 'raw' as const, smart: undefined, kind: 'raw-completed' },
    {
      name: 'Smart',
      mode: 'smart' as const,
      smart: { process: () => Promise.resolve('polished transcript') },
      kind: 'smart-completed',
    },
    { name: 'Smart fallback', mode: 'smart' as const, smart: undefined, kind: 'smart-fallback' },
  ])(
    'records $name exactly once at paste commit despite duplicate commit and restoration failure',
    async ({ mode, smart, kind }) => {
      const test = fixture({
        ...(smart === undefined ? {} : { smartProcessor: smart }),
        insert: (_text, _signal, onCommitted) => {
          onCommitted?.();
          onCommitted?.();
          return Promise.reject(new Error('clipboard restoration failed'));
        },
      });
      await test.controller.updateProfile('general', { processingMode: mode });
      test.notify(activation('down'));
      await settle();
      test.frame();
      test.notify(key('enter'));
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
      expect(test.controller.snapshot.completion).toBe('inserted');
      expect(test.spies.historyRecord).toHaveBeenCalledOnce();
      expect(test.spies.historyRecord).toHaveBeenCalledWith(
        expect.objectContaining({ kind, rawText: 'locally transcribed' }),
      );
      await test.controller.shutdown();
    },
  );

  it.each([
    ['UNAVAILABLE', 'pi-unavailable'],
    ['AUTHENTICATION_FAILED', 'pi-authentication-failed'],
    ['MODEL_NOT_FOUND', 'pi-model-not-found'],
    ['NO_MODELS', 'pi-no-models'],
    ['TIMEOUT', 'pi-timeout'],
    ['INVALID_RESPONSE', 'pi-invalid-response'],
    ['REMOTE_FAILURE', 'pi-remote-failure'],
  ] as const)(
    'preserves Pi fallback category %s through snapshot and history',
    async (code, category) => {
      const test = fixture({
        smartProcessor: {
          beginSession: () => ({
            providerId: 'pi',
            modelId: 'openai/gpt-test',
            prepare: () => Promise.resolve(),
            process: () => Promise.reject(new ProviderError(code)),
            commitScreenshot: () => undefined,
            cleanup: () => undefined,
          }),
        },
      });
      await test.controller.updateProfile('general', { processingMode: 'smart' });
      test.notify(activation('down'));
      await settle();
      test.frame();
      test.notify(key('enter'));
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
      expect(test.controller.snapshot.fallbackCategory).toBe(category);
      expect(test.spies.historyRecord).toHaveBeenCalledWith(
        expect.objectContaining({ kind: 'smart-fallback', errorCategory: category }),
      );
    },
  );

  it('records clipboard-only fallback only after insertion completion', async () => {
    const paste = deferred<{ inserted: boolean; copied: boolean }>();
    const test = fixture({ insert: () => paste.promise });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('inserting'));
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
    paste.resolve({ inserted: false, copied: true });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.completion).toBe('copied');
    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
  });

  it('submits immediately from arming instead of only classifying Quick', async () => {
    const test = fixture();
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    expect(test.controller.snapshot).toMatchObject({
      phase: 'transcribing',
      dictationMode: 'quick',
    });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
  });

  it('cancels with Esc and inserts nothing', async () => {
    const test = fixture();
    test.notify(activation('down'));
    await settle();
    test.notify(key('escape'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('cancelled'));
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
  });

  it('selects Extended after 600 ms and ignores shortcut release', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.notify(activation('down', true));
    await vi.advanceTimersByTimeAsync(600);
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingExtended',
      dictationMode: 'extended',
      processingMode: 'smart',
    });
    test.notify(activation('up', true));
    expect(test.controller.snapshot.phase).toBe('recordingExtended');
    test.controller.cancel();
    await vi.runAllTimersAsync();
  });

  it('classifies a 600 ms shortcut release as Extended when the hold callback is delayed', async () => {
    vi.useFakeTimers();
    vi.setSystemTime(1_000);
    const test = fixture();
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    vi.setSystemTime(1_600);
    test.notify(activation('up'));
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingExtended',
      dictationMode: 'extended',
      elapsedMs: 600,
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(test.spies.startSession).toHaveBeenCalledOnce();
    await test.controller.shutdown();
  });

  it('runs a helper-driven Quick gesture test without capture or insertion', async () => {
    vi.useFakeTimers();
    const test = fixture();
    const state = test.controller.startActivationTest(42, () => () => undefined);
    expect(state.phase).toBe('waiting');
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(599);
    expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false);
    test.notify(activation('up'));
    expect(test.controller.snapshot.phase).toBe('idle');
    expect(test.spies.startDictation).not.toHaveBeenCalled();
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.controller.stopActivationTest(42).phase).toBe('idle');
  });

  it.each([
    {
      name: 'helper unavailable',
      expected: 'helper-unavailable' as const,
      setup: () =>
        fixture({
          helperReadiness: {
            status: 'unavailable',
            reason: 'hook-fault',
            helperVersion: '1.0.0',
            permissions: {
              accessibility: 'not_applicable',
              inputMonitoring: 'not_applicable',
              eventPost: 'not_applicable',
            },
          },
        }),
    },
    {
      name: 'app disabled',
      expected: 'app-disabled' as const,
      setup: () => fixture(),
    },
    {
      name: 'session active',
      expected: 'session-active' as const,
      setup: () => fixture(),
    },
  ])('returns an actionable live-test refusal when $name', async ({ name, expected, setup }) => {
    const test = setup();
    if (name === 'app disabled') {
      await test.controller.updateGeneral({ app: { enabled: false } });
    } else if (name === 'session active') {
      test.notify(activation('down'));
      await settle();
    }
    expect(test.controller.startActivationTest(42, () => () => undefined)).toMatchObject({
      active: false,
      unavailableReason: expected,
    });
    await test.controller.shutdown();
  });

  it('reports an Extended helper gesture at the exact hold threshold without a session', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.controller.startActivationTest(42, () => () => undefined);
    test.notify(activation('down', true));
    await vi.advanceTimersByTimeAsync(600);
    test.notify(activation('up', true));
    expect(test.controller.activationTestState).toMatchObject({
      active: true,
      phase: 'extended',
      profileId: 'prompt',
      activationKey: 'Z',
      shift: true,
    });
    expect(test.spies.startDictation).not.toHaveBeenCalled();
    expect(test.controller.stopActivationTest(42).phase).toBe('idle');
  });

  it('submits Quick Dictation after armed trailing silence', async () => {
    const test = fixture();
    test.notify(activation('down'));
    await settle();
    test.notify(activation('up'));
    for (let index = 0; index < 15; index += 1) test.frame(undefined, 0.2);
    for (let index = 0; index < 90; index += 1) test.frame(undefined, 0.001);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
  });

  it('submits Quick Dictation at its duration cap', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(100);
    test.frame();
    test.notify(activation('up'));
    await vi.advanceTimersByTimeAsync(120_000);
    await vi.advanceTimersByTimeAsync(0);
    expect(['transcribing', 'inserting', 'completed']).toContain(test.controller.snapshot.phase);
    await test.controller.shutdown();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('finishes Extended Dictation from Stop', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.frame();
    test.controller.stop();
    await vi.advanceTimersByTimeAsync(0);
    await Promise.resolve();
    await vi.advanceTimersByTimeAsync(0);
    expect(test.spies.finish).toHaveBeenCalledOnce();
    await test.controller.shutdown();
  });

  it.each(['helper', 'capture'] as const)('tears down on %s loss', async (source) => {
    const test = fixture();
    test.notify(activation('down'));
    await settle();
    if (source === 'capture') test.loseCapture();
    else {
      test.setHelperReadiness({
        status: 'unavailable',
        reason: 'hook-fault',
        helperVersion: '1.0.0',
        permissions: {
          accessibility: 'not_applicable',
          inputMonitoring: 'not_applicable',
          eventPost: 'not_applicable',
        },
      });
    }
    await vi.waitFor(() =>
      expect(['cancelled', 'error']).toContain(test.controller.snapshot.phase),
    );
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.spies.stopDictation).toHaveBeenCalled();
  });

  it('gates stale activation notifications on current enabled, model, and helper readiness', async () => {
    let modelReady = false;
    const test = fixture({ isModelReady: () => modelReady });
    test.notify(activation('down'));
    expect(test.controller.snapshot.phase).toBe('idle');
    expect(test.spies.startDictation).not.toHaveBeenCalled();
    modelReady = true;
    await test.controller.updateGeneral({ app: { enabled: false } });
    test.notify(activation('down'));
    expect(test.controller.snapshot.phase).toBe('idle');
    await test.controller.updateGeneral({ app: { enabled: true } });
    test.helper.readiness.status = 'unavailable';
    test.notify(activation('down'));
    expect(test.controller.snapshot.phase).toBe('idle');
    await test.controller.shutdown();
  });

  it('coalesces model progress readiness while preserving the final helper hook state', async () => {
    let modelReady = false;
    const firstConfiguration: { resolve: (() => void) | null } = { resolve: null };
    let calls = 0;
    const configureActivation = vi.fn<EchoHelperPort['configureActivation']>(
      (enabled, bindings) => {
        calls += 1;
        if (calls === 1) {
          return new Promise((resolve) => {
            firstConfiguration.resolve = () => resolve({ enabled, bindings });
          });
        }
        return Promise.resolve({ enabled, bindings });
      },
    );
    const test = fixture({ isModelReady: () => modelReady, configureActivation });
    await vi.waitFor(() => expect(configureActivation).toHaveBeenCalledOnce());
    modelReady = true;
    test.controller.readinessChanged();
    test.controller.readinessChanged();
    test.controller.readinessChanged();
    if (firstConfiguration.resolve === null)
      throw new Error('Initial activation sync was not pending');
    firstConfiguration.resolve();
    await vi.waitFor(() => expect(configureActivation).toHaveBeenCalledTimes(2));
    expect(configureActivation.mock.calls.map(([enabled]) => enabled)).toEqual([false, true]);
    await test.controller.shutdown();
  });

  it('falls back to raw when the Smart extension is unavailable', async () => {
    const test = fixture();
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.abortReason).toBe('provider-error');
    expect(test.spies.insert).toHaveBeenCalledWith(
      'locally transcribed',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: 'smart-fallback',
        rawText: 'locally transcribed',
        errorCategory: 'provider-error',
      }),
    );
  });

  it('cleans retained Smart screenshots when the history transaction fails', async () => {
    const cleanup = vi.fn();
    const commitScreenshot = vi.fn();
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'generic-openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: () => Promise.resolve({ text: 'polished', screenshotFilename: 'retained.jpg' }),
          commitScreenshot,
          cleanup,
        }),
      },
      historyRecord: () => {
        throw new Error('database unavailable');
      },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(cleanup).toHaveBeenCalled();
    expect(commitScreenshot).not.toHaveBeenCalled();
  });

  it('prepares one submit-time screenshot before pending Whisper work can change the front app', async () => {
    const smartSettings = SettingsSchema.parse({
      ...structuredClone(DEFAULT_SETTINGS),
      smartProcessing: {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        selectedProviderId: 'openai',
        providers: { openai: { modelId: 'gpt-4.1' } },
        onScreenAwarenessEnabled: true,
      },
    });
    let frontBounds = { x: 10, y: 20, width: 300, height: 200 };
    const capture = vi.fn((bounds: typeof frontBounds) =>
      Promise.resolve({
        image: { mimeType: 'image/jpeg' as const, base64: '/9j/2Q==' },
        width: bounds.width,
        height: bounds.height,
      }),
    );
    const getFrontApp = vi.fn(() =>
      Promise.resolve({
        processName: 'target',
        windowTitle: 'document',
        windowBounds: frontBounds,
      }),
    );
    const smart = new SmartTranscriptionService({
      settings: {
        get: () => structuredClone(smartSettings),
        subscribe: () => () => undefined,
      } as unknown as SettingsStore,
      configs: {
        get: () => ({ providerId: 'openai', modelId: 'gpt-4.1' }),
        smartRevision: () => 0,
        subscribeSmartRevision: () => () => undefined,
      } as unknown as ProviderConfigService,
      providers: {
        credentialBinding: () => 'openai',
        capabilities: () => 'supported',
        preflightCapability: () => Promise.resolve('supported'),
        cleanTranscript: () => Promise.resolve('cleaned'),
      } as unknown as ProviderService,
      screenshots: { permissionStatus: () => 'granted', capture } as unknown as ScreenshotService,
      helper: { getFrontApp },
      screenshotsDirectory: 'unused',
    });
    const transcription = deferred<{
      text: string;
      modelId: 'Xenova/whisper-small';
      durationMs: number;
      pipeline: { loadCount: number; reused: boolean; loadDurationMs: number };
    }>();
    const test = fixture({
      smartProcessor: smart,
      transcribe: () => transcription.promise,
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(capture).toHaveBeenCalledOnce());
    expect(capture).toHaveBeenCalledWith(
      { x: 10, y: 20, width: 300, height: 200 },
      expect.anything(),
    );

    frontBounds = { x: 900, y: 700, width: 640, height: 480 };
    transcription.resolve({
      text: 'locally transcribed',
      modelId: 'Xenova/whisper-small',
      durationMs: 5,
      pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
    });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(getFrontApp).toHaveBeenCalledOnce();
    expect(capture).toHaveBeenCalledOnce();
  });

  it('runs a real timed provider through Smart service into raw insertion and timeout history', async () => {
    const smartSettings = SettingsSchema.parse({
      ...structuredClone(DEFAULT_SETTINGS),
      smartProcessing: {
        ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
        selectedProviderId: 'openai',
        providers: { openai: { modelId: 'gpt-4.1-nano' } },
      },
    });
    const transport: JsonTransport = {
      request: ({ signal }: JsonTransportRequest) =>
        new Promise<never>((_resolve, reject) => {
          const abort = () => reject(new ProviderError('CANCELLED'));
          if (signal.aborted) abort();
          else signal.addEventListener('abort', abort, { once: true });
        }),
      classify: () => Promise.resolve('cloud'),
    };
    const providers = new ProviderService(
      new ProviderRegistry({ transport }),
      { getCredential: () => 'local-test-key' },
      { operationTimeoutMs: 20 },
    );
    const smart = new SmartTranscriptionService({
      settings: {
        get: () => structuredClone(smartSettings),
        subscribe: () => () => undefined,
      } as unknown as SettingsStore,
      configs: {
        get: () => ({ providerId: 'openai', modelId: 'gpt-4.1-nano' }),
        smartRevision: () => 0,
        subscribeSmartRevision: () => () => undefined,
      } as unknown as ProviderConfigService,
      providers,
      screenshots: {
        permissionStatus: () => 'granted',
      } as unknown as ScreenshotService,
      helper: {
        getFrontApp: () => Promise.reject(new Error('OSA is disabled')),
      },
      screenshotsDirectory: 'unused',
    });
    const test = fixture({ smartProcessor: smart });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.abortReason).toBe('timeout');
    expect(test.spies.insert).toHaveBeenCalledWith(
      'locally transcribed',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({
        kind: 'smart-fallback',
        rawText: 'locally transcribed',
        errorCategory: 'timeout',
      }),
    );
    providers.dispose();
  });

  it.each(['timeout', 'provider-error'] as const)(
    'cancels obsolete Smart work but inserts raw fallback with a fresh signal on %s',
    async (reason) => {
      const smartOperation: { signal: AbortSignal | null } = { signal: null };
      const test = fixture({
        smartProcessor: {
          process: (_text, signal) => {
            smartOperation.signal = signal;
            return new Promise(() => undefined);
          },
        },
      });
      await test.controller.updateProfile('general', { processingMode: 'smart' });
      test.notify(activation('down'));
      await settle();
      test.frame();
      test.notify(activation('up'));
      test.notify(key('enter'));
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('processingSmart'));
      test.controller.abort(reason);
      await vi.waitFor(() => expect(test.spies.insert).toHaveBeenCalledOnce());
      const insertionSignal = test.spies.insert.mock.calls[0]?.[1];
      expect(smartOperation.signal?.aborted).toBe(true);
      expect(insertionSignal?.aborted).toBe(false);
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
      expect(test.controller.snapshot.abortReason).toBe(reason);
    },
  );

  it('fails and tears down without widget or start sound when helper capture enable fails', async () => {
    vi.useFakeTimers();
    let enableAttempts = 0;
    const test = fixture({
      setSessionCapture: (active) => {
        if (active && enableAttempts++ === 0) return Promise.reject(new Error('transient RPC'));
        return Promise.resolve({ active });
      },
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.showWidget).not.toHaveBeenCalled();
    expect(test.spies.sound).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    expect(test.spies.sound).toHaveBeenCalledOnce();
    test.controller.cancel();
    await test.controller.shutdown();
  });

  it('retries a stale enable failure for the newer session instead of failing it', async () => {
    vi.useFakeTimers();
    const firstEnable: { reject: ((error: Error) => void) | null } = { reject: null };
    let enableCalls = 0;
    const setSessionCapture = vi.fn((active: boolean) => {
      if (active && enableCalls++ === 0) {
        return new Promise<{ active: boolean }>((_resolve, reject) => {
          firstEnable.reject = reject;
        });
      }
      return Promise.resolve({ active });
    });
    const test = fixture({ setSessionCapture });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    if (firstEnable.reject === null) throw new Error('Old helper enable was not pending');
    firstEnable.reject(new Error('stale RPC failure'));
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    expect(test.controller.snapshot.phase).toBe('arming');
    expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([true, false, true]);
    test.controller.cancel();
    await test.controller.shutdown();
  });

  it('recovers an unknown native state by restarting the helper before idle', async () => {
    vi.useFakeTimers();
    let disableAttempts = 0;
    const setSessionCapture = vi.fn((active: boolean) => {
      if (!active && disableAttempts++ === 0) return Promise.reject(new Error('disable RPC'));
      return Promise.resolve({ active });
    });
    const test = fixture({ setSessionCapture });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('cancelled'));
    await vi.advanceTimersByTimeAsync(0);
    expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([true, false]);
    expect(test.spies.resetSessionCapture).toHaveBeenCalledOnce();
    await test.controller.shutdown();
  });

  it('does not let an old late helper-capture acknowledgement disable a newer session', async () => {
    vi.useFakeTimers();
    const firstTrue: { resolve: (() => void) | null } = { resolve: null };
    let trueCalls = 0;
    const setSessionCapture = vi.fn((active: boolean) => {
      if (active && trueCalls++ === 0) {
        return new Promise<{ active: boolean }>((resolve) => {
          firstTrue.resolve = () => resolve({ active: true });
        });
      }
      return Promise.resolve({ active });
    });
    const test = fixture({ setSessionCapture });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    expect(test.controller.snapshot.phase).toBe('cancelled');
    if (firstTrue.resolve === null) throw new Error('Initial helper capture was not pending');
    firstTrue.resolve();
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([true, false]);

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.setHelperReadiness(test.helper.readiness);
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([true, false, true]);

    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() =>
      expect(setSessionCapture.mock.calls.map(([active]) => active)).toEqual([
        true,
        false,
        true,
        false,
      ]),
    );
    await test.controller.shutdown();
  });

  it('shares one in-flight Extended stream opening across hold, audio push, and submit', async () => {
    vi.useFakeTimers();
    const opening = deferred<Awaited<ReturnType<WhisperWorkerClient['startSession']>>>();
    const push = vi.fn(() => Promise.resolve());
    const finish = vi.fn(() =>
      Promise.resolve({
        text: 'shared stream',
        modelId: 'Xenova/whisper-small' as const,
        durationMs: 1,
        pipeline: { loadCount: 1, reused: true, loadDurationMs: 0 },
      }),
    );
    const test = fixture({ startSession: () => opening.promise });

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.frame(new Float32Array(160_000).fill(0.2));
    test.controller.stop();
    await vi.advanceTimersByTimeAsync(0);
    expect(test.spies.startSession).toHaveBeenCalledOnce();

    opening.resolve({ id: 'shared', push, finish, cancel: () => Promise.resolve() });
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.startSession).toHaveBeenCalledOnce();
    expect(push).toHaveBeenCalledOnce();
    expect(finish).toHaveBeenCalledOnce();
  });

  it('fails closed instead of retaining unbounded Extended audio behind a slow stream', async () => {
    vi.useFakeTimers();
    const blocked = deferred<undefined>();
    const push = vi.fn(() => blocked.promise);
    const cancel = vi.fn(() => Promise.resolve());
    const test = fixture({
      startSession: () =>
        Promise.resolve({
          id: 'slow',
          push,
          finish: vi.fn(),
          cancel,
        }),
    });

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.frame(new Float32Array(160_000).fill(0.2));
    await vi.advanceTimersByTimeAsync(0);
    expect(push).toHaveBeenCalledOnce();
    test.frame(new Float32Array(160_001).fill(0.2));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(test.controller.snapshot.message).toContain('could not keep up');
    await vi.waitFor(() => expect(cancel).toHaveBeenCalledOnce());
  });

  it('handles an immediate Extended push rejection, tears down, and never finishes partial audio', async () => {
    vi.useFakeTimers();
    const push = vi.fn(() => Promise.reject(new Error('push failed')));
    const finish = vi.fn();
    const cancel = vi.fn(() => Promise.resolve());
    const test = fixture({
      startSession: () => Promise.resolve({ id: 'failed', push, finish, cancel }),
    });

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.frame(new Float32Array(160_000).fill(0.2));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(finish).not.toHaveBeenCalled();
    expect(cancel).toHaveBeenCalledOnce();
    expect(test.spies.stopDictation).toHaveBeenCalled();
  });

  it('confirms helper capture before microphone startup and releases the Quick model lease', async () => {
    const helperEnable = deferred<{ active: boolean }>();
    const release = vi.fn();
    const test = fixture({
      setSessionCapture: (active) =>
        active ? helperEnable.promise : Promise.resolve({ active: false }),
      acquireModelUse: () => Promise.resolve({ status: { state: 'ready' }, release }),
    });

    test.notify(activation('down'));
    await Promise.resolve();
    expect(test.spies.startDictation).not.toHaveBeenCalled();
    helperEnable.resolve({ active: true });
    await settle();
    expect(test.spies.startDictation).toHaveBeenCalledOnce();

    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(release).toHaveBeenCalledOnce();
  });

  it('reschedules idle after reset failure and later readiness recovery confirms capture off', async () => {
    vi.useFakeTimers();
    let disableAttempts = 0;
    const test = fixture({
      setSessionCapture: (active) => {
        if (!active && disableAttempts++ === 0) return Promise.reject(new Error('disable failed'));
        return Promise.resolve({ active });
      },
      resetSessionCapture: () => Promise.reject(new Error('restart failed')),
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(1_200);
    expect(test.controller.snapshot.phase).toBe('cancelled');

    test.setHelperReadiness(test.helper.readiness);
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
  });

  it('does not publish idle until failed helper disable is recovered by a bounded reset', async () => {
    vi.useFakeTimers();
    const reset = deferred<undefined>();
    const test = fixture({
      setSessionCapture: (active) =>
        active ? Promise.resolve({ active }) : Promise.reject(new Error('disable failed')),
      resetSessionCapture: () => reset.promise,
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(1_200);
    expect(test.controller.snapshot.phase).toBe('cancelled');

    reset.resolve(undefined);
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    expect(test.spies.resetSessionCapture).toHaveBeenCalledOnce();
  });

  it('shutdown races a hung capture startup and cleans up a late capture without UI', async () => {
    const startupControl: {
      resolve: ((capture: { captureId: string; activeMicrophoneId: string }) => void) | null;
    } = { resolve: null };
    const startup = new Promise<{ captureId: string; activeMicrophoneId: string }>((resolve) => {
      startupControl.resolve = resolve;
    });
    const test = fixture({ startDictation: () => startup });
    test.notify(activation('down'));
    await settle();
    await expect(test.controller.shutdown()).resolves.toBeUndefined();
    expect(test.spies.showWidget).not.toHaveBeenCalled();
    if (startupControl.resolve === null) throw new Error('Capture startup was not invoked');
    startupControl.resolve({ captureId: 'late-capture', activeMicrophoneId: 'default' });
    await vi.waitFor(() => expect(test.spies.stopDictation).toHaveBeenCalledWith('late-capture'));
    expect(test.spies.showWidget).not.toHaveBeenCalled();
  });

  it('treats an unexpected AbortError from an active operation as failure with teardown', async () => {
    const unexpectedAbort = new Error('worker aborted unexpectedly');
    unexpectedAbort.name = 'AbortError';
    const test = fixture({ transcribe: () => Promise.reject(unexpectedAbort) });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(test.controller.snapshot.message).toBe('Dictation could not be completed.');
    expect(test.spies.stopDictation).toHaveBeenCalled();
  });

  it('aborts and fully tears down while insertion is in flight', async () => {
    const insertionControl: { release: (() => void) | null } = { release: null };
    const insertionResult = new Promise<{ inserted: boolean; copied: boolean }>((resolve) => {
      insertionControl.release = () => resolve({ inserted: true, copied: false });
    });
    const test = fixture({ insert: () => insertionResult });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.spies.insert).toHaveBeenCalledOnce());
    const signal = test.spies.insert.mock.calls[0]?.[1];
    const shutdown = test.controller.shutdown();
    expect(signal?.aborted).toBe(true);
    if (insertionControl.release === null) throw new Error('Insertion was not started');
    insertionControl.release();
    await shutdown;
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false);
  });

  it.each([
    { committed: false, completion: 'copied' as const },
    { committed: true, completion: 'inserted' as const },
  ])(
    'bounds a hung insertion port with committed=$committed',
    async ({ committed, completion }) => {
      vi.useFakeTimers();
      let onCommitted: (() => void) | undefined;
      const test = fixture({
        insert: (_text, _signal, commit) => {
          onCommitted = commit;
          return new Promise(() => undefined);
        },
      });
      test.notify(activation('down'));
      await vi.advanceTimersByTimeAsync(1);
      test.frame();
      test.notify(key('enter'));
      await vi.advanceTimersByTimeAsync(1);
      expect(test.spies.insert).toHaveBeenCalledOnce();
      if (committed) onCommitted?.();
      await vi.advanceTimersByTimeAsync(5_000);
      expect(test.controller.snapshot).toMatchObject({ phase: 'completed', completion });
      await test.controller.shutdown();
    },
  );

  it('keeps in-flight history revocation one-way when it is enabled again before commit', async () => {
    const insertion = deferred<{ inserted: boolean; copied: boolean }>();
    let commit: (() => void) | undefined;
    const test = fixture({
      insert: (_text, _signal, onCommitted) => {
        commit = onCommitted;
        return insertion.promise;
      },
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.spies.insert).toHaveBeenCalledOnce());
    test.setPrivacy({ historyEnabled: false });
    test.setPrivacy({ historyEnabled: true });
    commit?.();
    insertion.resolve({ inserted: true, copied: false });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
  });

  it('shutdown clears active capture, streaming work, gesture and terminal timers', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.controller.startActivationTest(42, () => () => undefined);
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(100);
    test.controller.stopActivationTest(42);
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    await test.controller.shutdown();
    expect(test.spies.stopDictation).toHaveBeenCalled();
    expect(test.spies.cancelStream).toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('keeps successful insertion authoritative when optional history persistence fails', async () => {
    const test = fixture({
      historyRecord: () => {
        throw new Error('database unavailable');
      },
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.completion).toBe('inserted');
    expect(test.spies.historyRecord).toHaveBeenCalledOnce();
  });

  it('synchronizes helper activation through the profile API', async () => {
    const test = fixture();
    const result = await test.controller.updateProfile('general', {
      activationKey: 'Q',
      shift: false,
    });
    expect(
      result.dictationProfiles.find((profile) => profile.id === 'general')?.activationKey,
    ).toBe('Q');
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(true, [
      { key: 'Q', shift: false },
      { key: 'Z', shift: true },
    ]);
  });
});
