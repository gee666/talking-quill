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
import type { ActivationBinding, HelperNotification } from '../../app/src/shared/helper/protocol';
import type { HelperReadiness } from '../../app/src/shared/schemas/helper-readiness';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_MARKDOWN_PROFILE,
  DEFAULT_PROMPT_PROFILE,
  DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE,
} from '../../app/src/shared/schemas/dictation-profiles';
import {
  DEFAULT_SETTINGS,
  SettingsSchema,
  type Settings,
  type SettingsPatch,
} from '../../app/src/shared/schemas/settings';
import {
  shortcutFromLegacyActivation,
  type Shortcut,
  type ShortcutKey,
} from '../../app/src/shared/schemas/shortcut';

const profileBinding = (profileId: string, shortcut: Shortcut) => ({ profileId, shortcut });
const builtInBindings = (
  generalShortcut: Shortcut = DEFAULT_GENERAL_PROFILE.shortcut,
): ActivationBinding[] =>
  DEFAULT_SETTINGS.dictationProfiles.map((profile) =>
    profileBinding(profile.id, profile.id === 'general' ? generalShortcut : profile.shortcut),
  );

function fixture(
  options: {
    readonly insert?: InsertionService['insert'];
    readonly helperReadiness?: HelperReadiness;
    readonly startDictation?: RecordingService['startDictation'];
    readonly stopDictation?: RecordingService['stopDictation'];
    readonly getFrontApp?: EchoHelperPort['getFrontApp'];
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
    readonly settingsUpdateFailure?: (patch: SettingsPatch) => Error | null;
    readonly historyRecord?: EchoHistoryPort['record'];
  } = {},
) {
  let notification: ((value: HelperNotification) => void) | null = null;
  let readinessListener: ((value: HelperReadiness) => void) | null = null;
  let captureCallbacks: DictationCaptureCallbacks | null = null;
  const captureCallbackHistory: DictationCaptureCallbacks[] = [];
  let current = structuredClone(DEFAULT_SETTINGS);
  current.dictationProfiles = current.dictationProfiles.map((profile) =>
    profile.id === 'general' ? { ...profile, processingMode: 'raw' as const } : profile,
  );
  current.app.defaultProcessingMode = 'raw';
  const settingsListeners = new Set<(settings: Settings) => void>();
  const settings = {
    get: () => structuredClone(current),
    update: (patch: SettingsPatch) => {
      const failure = options.settingsUpdateFailure?.(patch) ?? null;
      if (failure !== null) return Promise.reject(failure);
      const dictationProfiles = patch.dictationProfiles ?? current.dictationProfiles;
      const general = dictationProfiles.find((profile) => profile.id === 'general');
      if (general === undefined) throw new Error('General profile is missing');
      current = SettingsSchema.parse({
        ...current,
        app: {
          ...current.app,
          ...defined(patch.app),
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
  const getFrontApp = vi.fn(
    options.getFrontApp ??
      (() =>
        Promise.resolve({
          processName: 'target',
          windowTitle: 'Target',
          windowBounds: { x: 100, y: 200, width: 800, height: 600 },
        })),
  );
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
    getFrontApp,
  } as unknown as HelperClient;
  const startDictation = vi.fn((callbacks: DictationCaptureCallbacks) => {
    captureCallbacks = callbacks;
    captureCallbackHistory.push(callbacks);
    return (
      options.startDictation?.(callbacks) ??
      Promise.resolve({
        captureId: '00000000-0000-4000-8000-000000000010',
        activeMicrophoneId: 'default',
      })
    );
  });
  const stopDictation = vi.fn(options.stopDictation ?? (() => Promise.resolve()));
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
  const showWidget = vi.fn(() => true);
  const showMain = vi.fn();
  const windows = {
    showWidget,
    hideWidget: vi.fn(),
    showMain,
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
      getFrontApp,
      configureActivation,
      setSessionCapture,
      resetSessionCapture,
      startSession,
      finish,
      cancelStream,
      showWidget,
      showMain,
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
    frameFromCapture(index: number, samples = new Float32Array(320).fill(0.2), rms = 0.2) {
      captureCallbackHistory[index]?.onFrame(samples, rms);
    },
    loseCaptureFrom(index: number) {
      captureCallbackHistory[index]?.onUnexpectedStop('device-unavailable');
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
  keyValue?: ShortcutKey,
  profileId = shift ? 'prompt' : 'general',
): HelperNotification {
  const defaultProfile = DEFAULT_SETTINGS.dictationProfiles.find(({ id }) => id === profileId);
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: {
      phase,
      profileId,
      shortcut:
        keyValue === undefined && defaultProfile !== undefined
          ? defaultProfile.shortcut
          : shortcutFromLegacyActivation(keyValue ?? 'Z', shift),
    },
  };
}
function activationComplete(
  heldMs: number,
  shortcut: Shortcut = DEFAULT_GENERAL_PROFILE.shortcut,
  profileId = 'general',
): HelperNotification {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase: 'complete', profileId, shortcut, heldMs },
  };
}
function chordActivation(
  phase: 'down' | 'up',
  shortcut: Shortcut,
  profileId = 'general',
): HelperNotification {
  return {
    jsonrpc: '2.0',
    method: 'activation.event',
    params: { phase, profileId, shortcut },
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

  it('isolates subscribers and still schedules effects after state publication', async () => {
    const test = fixture();
    const laterSubscriber = vi.fn();
    test.controller.subscribe(() => {
      throw new Error('subscriber failed');
    });
    test.controller.subscribe(laterSubscriber);

    test.notify(activation('down'));
    await settle();

    expect(laterSubscriber).toHaveBeenCalled();
    expect(test.spies.startDictation).toHaveBeenCalledOnce();
  });

  it('shows an actionable widget error when native shortcut registration fails', async () => {
    vi.useFakeTimers();
    const test = fixture({
      configureActivation: () => Promise.reject(new Error('invalid native bindings')),
    });

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('error'));
    expect(test.controller.snapshot.message).toBe(
      'Keyboard shortcuts could not be enabled. Restart Talking Quill or reinstall the app.',
    );
    expect(test.spies.showWidget).toHaveBeenCalledWith('default', null);
    await test.controller.shutdown();
  });

  it('shows the main window when an error widget renderer is unavailable', async () => {
    const test = fixture();
    await settle();
    test.spies.showWidget.mockReturnValue(false);

    test.setHelperReadiness({
      status: 'unavailable',
      reason: 'hook-fault',
      helperVersion: '1.0.0',
      permissions: test.helper.readiness.permissions,
    });

    expect(test.controller.snapshot.phase).toBe('error');
    expect(test.spies.showMain).toHaveBeenCalledOnce();
  });

  it('disables native activation during shortcut capture and restores the latest bindings', async () => {
    const test = fixture();
    let destroyed: (() => void) | null = null;
    const onDestroyed = (listener: () => void) => {
      destroyed = listener;
      return () => {
        destroyed = null;
      };
    };
    await settle();
    test.spies.configureActivation.mockClear();

    await test.controller.startShortcutCapture(7, onDestroyed);
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(false, builtInBindings());

    test.notify(activation('down'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('idle');
    expect(test.spies.startDictation).not.toHaveBeenCalled();

    await test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('Q', false),
    });
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      false,
      expect.arrayContaining([profileBinding('general', shortcutFromLegacyActivation('Q', false))]),
    );

    await test.controller.stopShortcutCapture(7);
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      true,
      expect.arrayContaining([profileBinding('general', shortcutFromLegacyActivation('Q', false))]),
    );
    expect(destroyed).toBeNull();
  });

  it('idempotently revokes invalidated capture owners and restores only after the last owner', async () => {
    const test = fixture();
    const invalidations = new Map<number, () => void>();
    const register = (owner: number) => (listener: () => void) => {
      invalidations.set(owner, listener);
      return () => invalidations.delete(owner);
    };
    await settle();
    await test.controller.startShortcutCapture(7, register(7));
    await test.controller.startShortcutCapture(8, register(8));
    await test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('Q', false),
    });
    test.spies.configureActivation.mockClear();

    const invalidateFirst = invalidations.get(7);
    if (invalidateFirst === undefined) throw new Error('First invalidation listener is missing');
    invalidateFirst();
    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledOnce());
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      false,
      expect.arrayContaining([profileBinding('general', shortcutFromLegacyActivation('Q', false))]),
    );

    const invalidateLast = invalidations.get(8);
    if (invalidateLast === undefined) throw new Error('Last invalidation listener is missing');
    invalidateLast();
    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledTimes(2));
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      true,
      expect.arrayContaining([profileBinding('general', shortcutFromLegacyActivation('Q', false))]),
    );

    invalidateFirst();
    invalidateLast();
    await settle();
    expect(test.spies.configureActivation).toHaveBeenCalledTimes(2);
  });

  it('retries authoritative activation after shortcut-capture restoration fails', async () => {
    const test = fixture();
    await settle();
    await test.controller.startShortcutCapture(8, () => () => undefined);
    test.spies.configureActivation.mockClear();
    let failed = false;
    test.spies.configureActivation.mockImplementation((enabled, bindings) => {
      if (enabled && !failed) {
        failed = true;
        return Promise.reject(new Error('transient registration failure'));
      }
      return Promise.resolve({ enabled, bindings });
    });

    await expect(test.controller.stopShortcutCapture(8)).rejects.toThrow(
      'transient registration failure',
    );

    await vi.waitFor(() => {
      expect(test.spies.configureActivation.mock.calls.filter(([enabled]) => enabled)).toHaveLength(
        2,
      );
    });
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(true, builtInBindings());
  });

  it('keeps the processing mirror atomic and resets each built-in to its reserved default', async () => {
    const test = fixture();
    let settings = await test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('G', true),
      processingMode: 'smart',
    });
    expect(settings.app).toMatchObject({ defaultProcessingMode: 'smart' });
    expect(settings.app).not.toHaveProperty('activationKey');
    settings = await test.controller.updateProfile('prompt', {
      shortcut: shortcutFromLegacyActivation('P', false),
      processingMode: 'raw',
    });
    expect(settings.app).toMatchObject({ defaultProcessingMode: 'smart' });

    await expect(
      test.controller.createProfile({
        name: 'Reserved thief',
        shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
        processingMode: 'raw',
        smartPrompt: null,
      }),
    ).rejects.toThrow(/reserved/i);

    settings = await test.controller.resetProfile('general');
    expect(settings.dictationProfiles.find((profile) => profile.id === 'general')).toEqual(
      DEFAULT_GENERAL_PROFILE,
    );
    expect(settings.app).toMatchObject({ defaultProcessingMode: 'smart' });
    settings = await test.controller.resetProfile('prompt');
    expect(settings.dictationProfiles.find((profile) => profile.id === 'prompt')).toEqual(
      DEFAULT_PROMPT_PROFILE,
    );
    settings = await test.controller.resetProfile('markdown');
    expect(settings.dictationProfiles.find((profile) => profile.id === 'markdown')).toEqual(
      DEFAULT_MARKDOWN_PROFILE,
    );
    settings = await test.controller.resetProfile('translate-to-english');
    expect(
      settings.dictationProfiles.find((profile) => profile.id === 'translate-to-english'),
    ).toEqual(DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE);
    expect(settings.app).toMatchObject({ defaultProcessingMode: 'smart' });
  });

  it('rejects built-in deletion and custom reset at the profile coordinator boundary', async () => {
    const test = fixture();
    const created = await test.controller.createProfile({
      name: 'Custom boundary profile',
      shortcut: shortcutFromLegacyActivation('Q', false),
      processingMode: 'smart',
      smartPrompt: null,
    });
    const custom = created.dictationProfiles.find(({ name }) => name === 'Custom boundary profile');
    if (custom === undefined) throw new Error('Custom profile was not created');
    test.spies.configureActivation.mockClear();

    await expect(test.controller.deleteProfile('general')).rejects.toThrow(
      'Built-in dictation profiles cannot be deleted',
    );
    await expect(test.controller.resetProfile(custom.id)).rejects.toThrow(
      'Only built-in profiles can be reset',
    );
    expect(test.spies.configureActivation).not.toHaveBeenCalled();

    const authoritative = await test.controller.updateProfile(custom.id, {
      name: 'Custom profile still present',
    });
    expect(authoritative.dictationProfiles.find(({ id }) => id === 'general')).toBeDefined();
    expect(authoritative.dictationProfiles.find(({ id }) => id === custom.id)?.name).toBe(
      'Custom profile still present',
    );
  });

  it('serializes complete profile CRUD transactions without losing concurrent updates', async () => {
    const firstConfiguration = deferred<{
      enabled: boolean;
      bindings: readonly ActivationBinding[];
    }>();
    const test = fixture();
    await settle();
    test.spies.configureActivation.mockClear();
    test.spies.configureActivation
      .mockImplementationOnce(() => firstConfiguration.promise)
      .mockImplementation((enabled, bindings) => Promise.resolve({ enabled, bindings }));

    const first = test.controller.createProfile({
      name: 'First concurrent profile',
      shortcut: shortcutFromLegacyActivation('Q', false),
      processingMode: 'raw',
      smartPrompt: null,
    });
    const second = test.controller.createProfile({
      name: 'Second concurrent profile',
      shortcut: shortcutFromLegacyActivation('R', false),
      processingMode: 'smart',
      smartPrompt: 'Second prompt.',
    });

    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledTimes(1));
    firstConfiguration.resolve({
      enabled: true,
      bindings: [
        profileBinding('general', DEFAULT_GENERAL_PROFILE.shortcut),
        profileBinding('prompt', DEFAULT_PROMPT_PROFILE.shortcut),
        profileBinding(
          '00000000-0000-4000-8000-000000000001',
          shortcutFromLegacyActivation('Q', false),
        ),
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
        expect.objectContaining({ shortcut: shortcutFromLegacyActivation('Q', false) }),
        expect.objectContaining({ shortcut: shortcutFromLegacyActivation('R', false) }),
      ]),
    );
  });

  it('resolves a committed profile mutation when post-commit sync fails and later reconciles', async () => {
    const test = fixture();
    await settle();
    test.spies.configureActivation.mockClear();
    let configuration = 0;
    test.spies.configureActivation.mockImplementation((enabled, bindings) => {
      configuration += 1;
      if (configuration === 2) return Promise.reject(new Error('post-commit sync failed'));
      return Promise.resolve({ enabled, bindings });
    });

    const committed = await test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('Q', false),
    });

    expect(
      committed.dictationProfiles.find((profile) => profile.id === 'general')?.shortcut,
    ).toEqual(shortcutFromLegacyActivation('Q', false));
    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledTimes(3));
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      true,
      builtInBindings(shortcutFromLegacyActivation('Q', false)),
    );
  });

  it('compensates rejected helper configuration from authoritative settings', async () => {
    const test = fixture();
    await settle();
    test.spies.configureActivation.mockClear();
    test.spies.configureActivation
      .mockRejectedValueOnce(new Error('registration failed'))
      .mockImplementation((enabled, bindings) => Promise.resolve({ enabled, bindings }));

    await expect(
      test.controller.updateProfile('general', {
        shortcut: shortcutFromLegacyActivation('Q', false),
      }),
    ).rejects.toThrow('registration failed');

    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(true, builtInBindings());
  });

  it('serializes General and profile mutations and compensates failures from authoritative settings', async () => {
    const firstConfiguration = deferred<{
      enabled: boolean;
      bindings: readonly ActivationBinding[];
    }>();
    let rejected = false;
    const test = fixture({
      settingsUpdateFailure: (patch) => {
        if (!rejected && patch.app?.enabled === false) {
          rejected = true;
          return new Error('settings write failed');
        }
        return null;
      },
    });
    await settle();
    test.spies.configureActivation.mockClear();
    test.spies.configureActivation
      .mockImplementationOnce(() => firstConfiguration.promise)
      .mockImplementation((enabled, bindings) => Promise.resolve({ enabled, bindings }));

    const disable = test.controller.updateGeneral({ app: { enabled: false } });
    const moveProfile = test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('Q', false),
    });
    await vi.waitFor(() => expect(test.spies.configureActivation).toHaveBeenCalledOnce());
    firstConfiguration.resolve({
      enabled: false,
      bindings: [
        profileBinding('general', DEFAULT_GENERAL_PROFILE.shortcut),
        profileBinding('prompt', DEFAULT_PROMPT_PROFILE.shortcut),
      ],
    });

    await expect(disable).rejects.toThrow('settings write failed');
    const authoritative = await moveProfile;
    expect(authoritative.app.enabled).toBe(true);
    expect(
      authoritative.dictationProfiles.find((profile) => profile.id === 'general')?.shortcut,
    ).toEqual(shortcutFromLegacyActivation('Q', false));
    await vi.waitFor(() =>
      expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
        true,
        builtInBindings(shortcutFromLegacyActivation('Q', false)),
      ),
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
      shortcut: shortcutFromLegacyActivation('Q', false),
      processingMode: 'raw',
      smartPrompt: null,
    });
    const custom = withCustom.dictationProfiles.find((profile) => profile.name === 'Temporary');
    expect(custom).toBeDefined();
    if (custom === undefined) throw new Error('Temporary profile was not created');

    test.notify(activation('down', true));
    await settle();
    expect(test.controller.snapshot.processingMode).toBe('smart');
    expect(test.controller.snapshot.alternate).toBe(false);
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
        shortcut: DEFAULT_PROMPT_PROFILE.shortcut,
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
      shortcut: shortcutFromLegacyActivation('Q', true),
      processingMode: 'smart',
      smartPrompt: 'Deleted profile preference.',
    });
    const custom = settings.dictationProfiles.find((profile) => profile.name === 'Custom Smart');
    expect(custom).toBeDefined();
    if (custom === undefined) throw new Error('Custom Smart profile was not created');

    test.notify(activation('down', true, 'Q', custom.id));
    await settle();
    await test.controller.deleteProfile(custom.id);
    test.frame();
    test.notify(activation('up', true, 'Q', custom.id));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    expect(beginSession).toHaveBeenCalledWith(
      expect.objectContaining({ id: custom.id, smartPrompt: 'Deleted profile preference.' }),
    );
    expect(promptsUsed).toEqual(['Deleted profile preference.']);
  });

  it('selects exact profile-owned chords while General remains raw', async () => {
    const beginSession = vi.fn();
    const test = fixture({
      smartProcessor: { beginSession } as unknown as SmartTranscriptProcessor,
    });

    test.notify(activation('down', false, 'Q'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('idle');
    test.notify(activation('down', false, 'Z', 'prompt'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('idle');

    test.notify(activation('down'));
    await settle();
    expect(test.controller.snapshot.processingMode).toBe('raw');
    expect(test.controller.snapshot.alternate).toBe(false);
    test.frame();
    test.notify(activation('up'));
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

  it('cancels parallel submit-time Smart preparation and transcription, then discards context', async () => {
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
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
    expect(test.spies.transcribe.mock.calls[0]?.[2].aborted).toBe(true);
    expect(test.spies.insert).not.toHaveBeenCalled();
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
  });

  it('executes and records a command verified by Smart processing', async () => {
    const command = {
      id: '11111111-1111-4111-8111-111111111111',
      trigger: 'send report',
      snippet: 'inserted report',
      createdAt: 1,
      updatedAt: 1,
    };
    const cleanup = vi.fn();
    const test = fixture({
      commands: { match: () => ({ command, kind: 'fuzzy', score: 0.9 }) },
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: () =>
            Promise.resolve({
              text: command.trigger,
              screenshotFilename: null,
              voiceCommand: command,
            }),
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
    expect(test.spies.insert).toHaveBeenCalledWith(
      command.snippet,
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(cleanup).toHaveBeenCalledOnce();
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'voice-command', voiceTrigger: command.trigger }),
    );
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

  it('preserves a committed insertion before showing a later helper failure', async () => {
    vi.useFakeTimers();
    const restoration = deferred<undefined>();
    const test = fixture({
      insert: async (_text, _signal, onCommitted) => {
        onCommitted?.();
        await restoration.promise;
        return { inserted: true, copied: false };
      },
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('restoringClipboard'));

    test.setHelperReadiness({
      status: 'unavailable',
      reason: 'hook-fault',
      helperVersion: '1.0.0',
      permissions: test.helper.readiness.permissions,
    });
    expect(test.controller.snapshot.phase).toBe('restoringClipboard');

    restoration.resolve(undefined);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.message).toBe('Inserted');
    await vi.advanceTimersByTimeAsync(1_200);
    expect(test.controller.snapshot).toMatchObject({
      phase: 'error',
      message: 'Keyboard shortcuts are unavailable. Restart Talking Quill or reinstall the app.',
    });
    await test.controller.shutdown();
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

  it('forwards the configured non-English source language to one-shot transcription', async () => {
    const test = fixture();
    await test.controller.updateGeneral({ transcription: { language: 'ru' } });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    expect(test.spies.transcribe).toHaveBeenCalledWith(
      expect.any(Float32Array),
      {
        modelId: DEFAULT_SETTINGS.transcription.modelId,
        sampleRate: 16_000,
        language: 'ru',
      },
      expect.any(AbortSignal),
    );
  });

  it('forwards the configured non-English source language when opening a stream', async () => {
    vi.useFakeTimers();
    const test = fixture();
    await test.controller.updateGeneral({ transcription: { language: 'uk' } });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    await vi.advanceTimersByTimeAsync(0);

    expect(test.spies.startSession).toHaveBeenCalledWith(
      {
        modelId: DEFAULT_SETTINGS.transcription.modelId,
        sampleRate: 16_000,
        language: 'uk',
      },
      expect.any(AbortSignal),
    );
    test.controller.cancel();
    await vi.runAllTimersAsync();
  });

  it('runs Translate to English through Smart after source-language Whisper transcription', async () => {
    const process = vi.fn(() => Promise.resolve({ text: 'Hello world', screenshotFilename: null }));
    const beginSession = vi.fn<
      Extract<SmartTranscriptProcessor, { beginSession: unknown }>['beginSession']
    >(() => ({
      providerId: 'openai',
      modelId: 'gpt-4.1',
      prepare: () => Promise.resolve(),
      process,
      commitScreenshot: vi.fn(),
      cleanup: vi.fn(),
    }));
    const test = fixture({ smartProcessor: { beginSession } });
    await test.controller.updateGeneral({ transcription: { language: 'ru' } });

    test.notify(
      chordActivation(
        'down',
        DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut,
        'translate-to-english',
      ),
    );
    await settle();
    test.frame();
    test.notify(
      chordActivation('up', DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE.shortcut, 'translate-to-english'),
    );
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    expect(test.spies.transcribe).toHaveBeenCalledWith(
      expect.any(Float32Array),
      expect.objectContaining({ language: 'ru' }),
      expect.any(AbortSignal),
    );
    expect(beginSession).toHaveBeenCalledWith(DEFAULT_TRANSLATE_TO_ENGLISH_PROFILE);
    expect(process).toHaveBeenCalledWith('locally transcribed', expect.any(AbortSignal));
    expect(test.spies.insert).toHaveBeenCalledWith(
      'Hello world',
      expect.any(AbortSignal),
      expect.any(Function),
    );
  });

  it('keeps final capture frames that arrive while microphone stop is draining', async () => {
    const stopped = deferred<undefined>();
    const test = fixture({
      stopDictation: (captureId) => (captureId === undefined ? Promise.resolve() : stopped.promise),
    });
    test.notify(activation('down'));
    await settle();
    test.frame(new Float32Array(320).fill(0.1));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.spies.stopDictation).toHaveBeenCalledOnce());

    test.frame(new Float32Array(320).fill(0.2));
    stopped.resolve(undefined);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe.mock.calls[0]?.[0]).toHaveLength(640);
  });

  it('disarms helper capture without waiting for microphone stop acknowledgement', async () => {
    const stopped = deferred<undefined>();
    const test = fixture({
      stopDictation: (captureId) => (captureId === undefined ? Promise.resolve() : stopped.promise),
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));

    await vi.waitFor(() => expect(test.spies.stopDictation).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false));
    stopped.resolve(undefined);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
  });

  it('does not let terminal teardown stop a microphone test that followed its dictation', async () => {
    const transcription = deferred<Awaited<ReturnType<WhisperWorkerClient['transcribe']>>>();
    let microphoneTestActive = false;
    const test = fixture({
      stopDictation: (captureId) => {
        if (captureId === undefined) microphoneTestActive = false;
        return Promise.resolve();
      },
      transcribe: () => transcription.promise,
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() =>
      expect(test.spies.stopDictation).toHaveBeenCalledWith('00000000-0000-4000-8000-000000000010'),
    );

    microphoneTestActive = true;
    transcription.resolve({
      text: 'owner-safe transcription',
      modelId: 'Xenova/whisper-small',
      durationMs: 5,
      pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
    });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    await settle();

    expect(test.spies.stopDictation).not.toHaveBeenCalledWith(undefined);
    expect(microphoneTestActive).toBe(true);
    await test.controller.shutdown();
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

  it('waits for asynchronous capture startup when Enter arrives in the activation batch', async () => {
    const startup = deferred<{ captureId: string; activeMicrophoneId: string }>();
    const callbacks: { current: DictationCaptureCallbacks | null } = { current: null };
    const test = fixture({
      startDictation: (nextCallbacks) => {
        callbacks.current = nextCallbacks;
        return startup.promise;
      },
    });

    test.notify(activation('down'));
    test.notify(key('enter'));
    expect(test.controller.snapshot).toMatchObject({ phase: 'arming', dictationMode: 'quick' });
    await vi.waitFor(() => expect(test.spies.startDictation).toHaveBeenCalledOnce());
    expect(test.spies.stopDictation).not.toHaveBeenCalled();
    startup.resolve({ captureId: 'batched-capture', activeMicrophoneId: 'default' });
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    expect(test.controller.snapshot.phase).toBe('arming');
    expect(test.spies.transcribe).not.toHaveBeenCalled();

    callbacks.current?.onFrame(new Float32Array(320).fill(0.2), 0.2);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.stopDictation).toHaveBeenCalledWith('batched-capture');
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
  });

  it('keeps activation feedback when the first frame precedes capture startup acknowledgement', async () => {
    const startup = deferred<{ captureId: string; activeMicrophoneId: string }>();
    const callbacks: { current: DictationCaptureCallbacks | null } = { current: null };
    const test = fixture({
      startDictation: (nextCallbacks) => {
        callbacks.current = nextCallbacks;
        return startup.promise;
      },
    });
    test.notify(activation('down'));
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.spies.startDictation).toHaveBeenCalledOnce());
    callbacks.current?.onFrame(new Float32Array(320).fill(0.2), 0.2);
    startup.resolve({ captureId: 'early-frame', activeMicrophoneId: 'default' });

    await vi.waitFor(() => expect(test.spies.sound).toHaveBeenCalled());
    expect(test.spies.showWidget).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
  });

  it('fails safely when activated capture never produces its first audio frame', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.notify(key('enter'));

    await vi.advanceTimersByTimeAsync(1_000);
    expect(test.controller.snapshot).toMatchObject({
      phase: 'error',
      message: 'The microphone did not provide audio.',
    });
    expect(test.spies.transcribe).not.toHaveBeenCalled();
    await test.controller.shutdown();
  });

  it('rejects an idle down whose profile ID does not own the current shortcut', async () => {
    const test = fixture();
    const general = DEFAULT_GENERAL_PROFILE.shortcut;

    test.notify(chordActivation('down', general, 'prompt'));
    await settle();

    expect(test.controller.snapshot.phase).toBe('idle');
    expect(test.spies.startDictation).not.toHaveBeenCalled();
    expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false);
  });

  it('accepts an atomic completion for an exact built-in prefix binding', async () => {
    const test = fixture();
    test.notify(activationComplete(100, DEFAULT_PROMPT_PROFILE.shortcut, 'prompt'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('recordingQuick'));
    expect(test.controller.snapshot.processingMode).toBe('smart');
    expect(test.spies.startDictation).toHaveBeenCalledOnce();
    test.controller.cancel();
  });

  it.each([
    [599, 'recordingQuick', 'quick'],
    [600, 'recordingExtended', 'extended'],
  ] as const)(
    'classifies an atomic General prefix completion held for %i ms',
    async (heldMs, phase, mode) => {
      const test = fixture();
      test.notify(activationComplete(heldMs));
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe(phase));
      expect(test.controller.snapshot.dictationMode).toBe(mode);
      await vi.waitFor(() => expect(test.spies.startDictation).toHaveBeenCalledOnce());
    },
  );

  it('pairs Quick release with the exact full trigger-P shortcut snapshot', async () => {
    const test = fixture();
    const shortcut: Shortcut = {
      modifiers: { ctrl: true, alt: true, shift: false, meta: false },
      keys: ['Q', 'P'],
    };
    const wrong: Shortcut = {
      modifiers: { ctrl: true, alt: true, shift: false, meta: false },
      keys: ['R', 'P'],
    };
    await test.controller.updateProfile('general', { shortcut });

    test.notify(chordActivation('down', shortcut));
    await settle();
    test.frame();
    test.notify(chordActivation('up', shortcut, 'prompt'));
    expect(test.controller.snapshot.phase).toBe('arming');
    test.notify(chordActivation('up', wrong));
    expect(test.controller.snapshot.phase).toBe('arming');
    test.notify(chordActivation('up', shortcut));
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingQuick',
      dictationMode: 'quick',
      alternate: false,
    });
    test.notify(chordActivation('up', shortcut));
    expect(test.controller.snapshot.phase).toBe('recordingQuick');
    test.controller.cancel();
  });

  it('requires exact profile ownership for a recording shortcut submit', async () => {
    const test = fixture();
    const general = DEFAULT_GENERAL_PROFILE.shortcut;
    test.notify(chordActivation('down', general, 'general'));
    await settle();
    test.frame();
    test.notify(chordActivation('up', general, 'general'));
    expect(test.controller.snapshot.phase).toBe('recordingQuick');

    test.notify(chordActivation('down', general, 'prompt'));
    await settle();
    expect(test.controller.snapshot.phase).toBe('recordingQuick');
    expect(test.spies.transcribe).not.toHaveBeenCalled();

    test.notify(chordActivation('down', general, 'general'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
  });

  it('submits a frozen-profile recording with its edited shortcut and pairs the edited snapshot', async () => {
    const transcription = deferred<Awaited<ReturnType<WhisperWorkerClient['transcribe']>>>();
    const test = fixture({ transcribe: () => transcription.promise });
    const original = DEFAULT_GENERAL_PROFILE.shortcut;
    const edited = shortcutFromLegacyActivation('Q', false);
    test.notify(chordActivation('down', original, 'general'));
    await settle();
    test.frame();
    test.notify(chordActivation('up', original, 'general'));
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingQuick',
      dictationMode: 'quick',
    });
    await test.controller.updateProfile('general', { shortcut: edited });

    test.notify(chordActivation('down', edited, 'general'));
    expect(test.controller.snapshot.phase).toBe('transcribing');
    test.notify(chordActivation('up', original, 'general'));
    test.notify(chordActivation('up', edited, 'prompt'));
    test.notify(chordActivation('up', edited, 'general'));

    transcription.resolve({
      text: 'edited shortcut submission',
      modelId: 'Xenova/whisper-small',
      durationMs: 5,
      pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
    });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
  });

  it('does not submit when the frozen profile old shortcut is reassigned to another profile', async () => {
    const test = fixture();
    const original = shortcutFromLegacyActivation('G', false);
    const edited = shortcutFromLegacyActivation('Q', false);
    await test.controller.updateProfile('general', { shortcut: original });
    test.notify(chordActivation('down', original, 'general'));
    await settle();
    test.frame();
    test.notify(chordActivation('up', original, 'general'));
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingQuick',
      dictationMode: 'quick',
    });
    await test.controller.updateProfile('general', { shortcut: edited });
    const reassigned = await test.controller.createProfile({
      name: 'Reassigned old shortcut',
      shortcut: original,
      processingMode: 'raw',
      smartPrompt: null,
    });
    const reassignedProfile = reassigned.dictationProfiles.find(
      (profile) => profile.name === 'Reassigned old shortcut',
    );
    if (reassignedProfile === undefined) throw new Error('Reassigned profile is missing');

    test.notify(chordActivation('down', original, reassignedProfile.id));
    await settle();
    expect(test.controller.snapshot.phase).toBe('recordingQuick');
    expect(test.spies.transcribe).not.toHaveBeenCalled();

    test.notify(chordActivation('down', edited, 'general'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe).toHaveBeenCalledOnce();
  });

  it('accepts the frozen profile-owned up after its configured shortcut changes', async () => {
    const test = fixture();
    const original = DEFAULT_GENERAL_PROFILE.shortcut;

    test.notify(chordActivation('down', original, 'general'));
    await settle();
    test.frame();
    await test.controller.updateProfile('general', {
      shortcut: shortcutFromLegacyActivation('Q', false),
    });
    test.notify(chordActivation('up', original, 'general'));

    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingQuick',
      dictationMode: 'quick',
    });
    test.controller.cancel();
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

  it('selects Extended from an exact full trigger-P shortcut after 600 ms', async () => {
    vi.useFakeTimers();
    const test = fixture();
    const shortcut: Shortcut = {
      modifiers: { ctrl: true, alt: false, shift: true, meta: true },
      keys: ['Q', 'P'],
    };
    await test.controller.updateProfile('prompt', { shortcut });
    test.notify(chordActivation('down', shortcut, 'prompt'));
    await vi.advanceTimersByTimeAsync(600);
    expect(test.controller.snapshot).toMatchObject({
      phase: 'recordingExtended',
      dictationMode: 'extended',
      processingMode: 'smart',
      alternate: true,
    });
    test.notify(chordActivation('up', shortcut, 'prompt'));
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

  it('stops and releases an active activation test immediately when activation is disabled', async () => {
    const test = fixture();
    const removeOwner = vi.fn();
    const ownerDestroyed: { current: (() => void) | null } = { current: null };
    test.controller.startActivationTest(42, (listener) => {
      ownerDestroyed.current = listener;
      return removeOwner;
    });
    test.notify(activation('down'));
    expect(test.controller.activationTestState.phase).toBe('pressed');

    const disabling = test.controller.updateGeneral({ app: { enabled: false } });

    expect(test.controller.activationTestState).toMatchObject({ active: false, phase: 'idle' });
    expect(removeOwner).toHaveBeenCalledOnce();
    ownerDestroyed.current?.();
    expect(removeOwner).toHaveBeenCalledOnce();
    await expect(disabling).resolves.toMatchObject({ app: { enabled: false } });
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
      shortcut: DEFAULT_PROMPT_PROFILE.shortcut,
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

  it('falls back to raw when provider failure cleanup also throws', async () => {
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: () => Promise.reject(new ProviderError('REMOTE_FAILURE')),
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
    expect(test.controller.snapshot.abortReason).toBe('provider-error');
    expect(test.spies.insert).toHaveBeenCalledWith(
      'locally transcribed',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'smart-fallback', errorCategory: 'provider-error' }),
    );
    expect(cleanup).toHaveBeenCalledOnce();
  });

  it('reaches idle when cancellation cleanup throws during teardown', async () => {
    vi.useFakeTimers();
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: vi.fn(),
          commitScreenshot: vi.fn(),
          cleanup,
        }),
      },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(0);
    expect(test.controller.snapshot.phase).toBe('cancelled');
    expect(cleanup).toHaveBeenCalledOnce();

    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
  });

  it('keeps insertion successful when history-disabled Smart cleanup throws', async () => {
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: () => Promise.resolve({ text: 'polished', screenshotFilename: 'pending.jpg' }),
          commitScreenshot: vi.fn(),
          cleanup,
        }),
      },
    });
    await test.controller.updateProfile('general', { processingMode: 'smart' });
    test.setPrivacy({ historyEnabled: false });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));

    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.controller.snapshot.completion).toBe('inserted');
    expect(test.spies.insert).toHaveBeenCalledWith(
      'polished',
      expect.any(AbortSignal),
      expect.any(Function),
    );
    expect(test.spies.historyRecord).not.toHaveBeenCalled();
    expect(cleanup).toHaveBeenCalledOnce();
  });

  it('cleans retained Smart screenshots when the history transaction fails', async () => {
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
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
    expect(test.controller.snapshot.completion).toBe('inserted');
    expect(cleanup).toHaveBeenCalledOnce();
    expect(commitScreenshot).not.toHaveBeenCalled();
  });

  it('keeps committed history authoritative when screenshot finalization throws', async () => {
    const commitScreenshot = vi.fn(() => {
      throw new Error('commit failed');
    });
    const cleanup = vi.fn(() => {
      throw new Error('cleanup failed');
    });
    const test = fixture({
      smartProcessor: {
        beginSession: () => ({
          providerId: 'openai',
          modelId: 'gpt-4.1',
          prepare: () => Promise.resolve(),
          process: () => Promise.resolve({ text: 'polished', screenshotFilename: 'pending.jpg' }),
          commitScreenshot,
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
    expect(test.controller.snapshot.completion).toBe('inserted');
    expect(test.spies.historyRecord).toHaveBeenCalledWith(
      expect.objectContaining({ kind: 'smart-completed', screenshotFilename: 'pending.jpg' }),
    );
    expect(commitScreenshot).toHaveBeenCalledOnce();
    expect(cleanup).toHaveBeenCalledOnce();
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

  it('shows and sounds immediate arming feedback, then cleans up if helper capture enable fails', async () => {
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
    expect(test.spies.stopDictation).toHaveBeenCalledOnce();
    expect(test.spies.showWidget).toHaveBeenCalledOnce();
    expect(test.spies.sound).toHaveBeenCalledOnce();

    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledTimes(2));
    expect(test.spies.sound).toHaveBeenCalledTimes(2);
    test.controller.cancel();
    await test.controller.shutdown();
  });

  it('does not reset to idle while a terminal-phase activation is still being disarmed', async () => {
    vi.useFakeTimers();
    const terminalDisable = deferred<{ active: boolean }>();
    let blockDisable = false;
    const test = fixture({
      setSessionCapture: (active) =>
        !active && blockDisable ? terminalDisable.promise : Promise.resolve({ active }),
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));

    blockDisable = true;
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(1_200);
    expect(test.controller.snapshot.phase).toBe('completed');

    terminalDisable.resolve({ active: false });
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
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
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledTimes(2));
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

  it('reissues capture-off when activation re-arms native capture before an old acknowledgement', async () => {
    const firstDisable = deferred<{ active: boolean }>();
    const secondDisable = deferred<{ active: boolean }>();
    let disableCalls = 0;
    const test = fixture({
      setSessionCapture: (active) => {
        if (active) return Promise.resolve({ active });
        disableCalls += 1;
        return disableCalls === 1 ? firstDisable.promise : secondDisable.promise;
      },
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(disableCalls).toBe(1));
    expect(test.controller.snapshot.phase).toBe('transcribing');

    test.notify(activation('down'));
    firstDisable.resolve({ active: false });
    await vi.waitFor(() => expect(disableCalls).toBe(2));
    expect(test.controller.snapshot.phase).toBe('transcribing');

    secondDisable.resolve({ active: false });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    await test.controller.shutdown();
  });

  it('reconciles again when activation re-arms capture during helper-reset fallback', async () => {
    const reset = deferred<undefined>();
    let disableCalls = 0;
    const test = fixture({
      setSessionCapture: (active) => {
        if (active) return Promise.resolve({ active });
        disableCalls += 1;
        return disableCalls === 1
          ? Promise.reject(new Error('capture state unknown'))
          : Promise.resolve({ active: false });
      },
      resetSessionCapture: () => reset.promise,
    });
    test.notify(activation('down'));
    await settle();
    test.frame();
    test.notify(key('enter'));
    await vi.waitFor(() => expect(test.spies.resetSessionCapture).toHaveBeenCalledOnce());

    test.notify(activation('down'));
    reset.resolve(undefined);
    await vi.waitFor(() => expect(disableCalls).toBe(2));
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    await test.controller.shutdown();
  });

  it('ignores stale recording callbacks after a rapid cancel and restart', async () => {
    vi.useFakeTimers();
    const test = fixture();
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    expect(test.controller.snapshot.phase).toBe('arming');
    test.frameFromCapture(0, new Float32Array(320).fill(0.9), 0.9);
    test.loseCaptureFrom(0);
    expect(test.controller.snapshot.phase).toBe('arming');

    test.frame(new Float32Array(320).fill(0.2), 0.2);
    test.notify(activation('up'));
    test.notify(key('enter'));
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
    expect(test.spies.transcribe.mock.calls[0]?.[0]).toHaveLength(320);
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
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledTimes(2));
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

  it('aborts a hung Extended stream opening so failure teardown is not trapped behind it', async () => {
    vi.useFakeTimers();
    const startup: { signal: AbortSignal | null } = { signal: null };
    const test = fixture({
      startSession: (_options, signal) => {
        startup.signal = signal ?? null;
        return new Promise(() => undefined);
      },
    });

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    await vi.waitFor(() => expect(test.spies.startSession).toHaveBeenCalledOnce());

    test.loseCapture();
    expect(test.controller.snapshot).toMatchObject({
      phase: 'error',
      message: 'The microphone stopped unexpectedly.',
    });
    expect(startup.signal?.aborted).toBe(true);
    await vi.advanceTimersByTimeAsync(0);
    await vi.waitFor(() => expect(test.spies.stopDictation).toHaveBeenCalled());
    expect(test.spies.setSessionCapture).toHaveBeenLastCalledWith(false);

    await vi.advanceTimersByTimeAsync(1_200);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    await test.controller.shutdown();
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

  it('does not expose idle or a new session before bounded stream teardown completes', async () => {
    vi.useFakeTimers();
    const test = fixture({
      startSession: () =>
        Promise.resolve({
          id: 'hung-cancel',
          push: () => Promise.resolve(),
          finish: vi.fn(),
          cancel: () => new Promise(() => undefined),
        }),
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.controller.cancel();

    await vi.advanceTimersByTimeAsync(1_200);
    expect(test.controller.snapshot.phase).toBe('cancelled');
    await vi.advanceTimersByTimeAsync(1_000);
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));
    await test.controller.shutdown();
  });

  it.each(['resolve', 'reject'] as const)(
    'fences a stale Extended push %s after bounded teardown and a rapid Quick restart',
    async (settlement) => {
      vi.useFakeTimers();
      const stalePush = deferred<undefined>();
      const push = vi.fn(() => stalePush.promise);
      const test = fixture({
        startSession: () =>
          Promise.resolve({
            id: 'stale-generation',
            push,
            finish: vi.fn(),
            cancel: () => new Promise(() => undefined),
          }),
      });
      test.notify(activation('down'));
      await vi.advanceTimersByTimeAsync(600);
      test.frame(new Float32Array(160_000).fill(0.2));
      await vi.advanceTimersByTimeAsync(0);
      expect(push).toHaveBeenCalledOnce();
      test.controller.cancel();

      await vi.advanceTimersByTimeAsync(1_100);
      expect(test.controller.snapshot.phase).toBe('cancelled');
      await vi.advanceTimersByTimeAsync(1_000);
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('idle'));

      test.notify(activation('down'));
      await vi.advanceTimersByTimeAsync(0);
      const quickPcm = new Float32Array(160_000).fill(0.4);
      test.frame(quickPcm);
      test.notify(activation('up'));
      expect(test.controller.snapshot.phase).toBe('recordingQuick');

      if (settlement === 'resolve') stalePush.resolve(undefined);
      else stalePush.reject(new Error('stale push failed'));
      await vi.advanceTimersByTimeAsync(0);
      expect(test.controller.snapshot.phase).toBe('recordingQuick');

      test.notify(key('enter'));
      await vi.advanceTimersByTimeAsync(0);
      await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('completed'));
      expect(test.spies.transcribe.mock.calls[0]?.[0]).toHaveLength(quickPcm.length);
      await test.controller.shutdown();
    },
  );

  it('overlaps microphone startup with model acquisition and helper capture confirmation', async () => {
    const model = deferred<EchoModelUseGrant>();
    const helperEnable = deferred<{ active: boolean }>();
    const test = fixture({
      acquireModelUse: () => model.promise,
      setSessionCapture: (active) =>
        active ? helperEnable.promise : Promise.resolve({ active: false }),
    });

    test.notify(activation('down'));
    await vi.waitFor(() => expect(test.spies.setSessionCapture).toHaveBeenCalledWith(true));
    expect(test.spies.startDictation).toHaveBeenCalledOnce();
    expect(test.spies.sound).toHaveBeenCalledOnce();

    model.resolve({ status: { state: 'ready' }, release: vi.fn() });
    await Promise.resolve();
    expect(test.controller.snapshot.phase).toBe('arming');
    helperEnable.resolve({ active: true });
    await vi.waitFor(() => expect(test.controller.snapshot.phase).toBe('arming'));
    test.controller.cancel();
    await test.controller.shutdown();
  });

  it('bounds teardown when one parallel startup gate fails and the other never settles', async () => {
    vi.useFakeTimers();
    const test = fixture({
      acquireModelUse: () => new Promise(() => undefined),
      setSessionCapture: (active) =>
        active ? Promise.reject(new Error('capture enable failed')) : Promise.resolve({ active }),
    });

    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    expect(test.controller.snapshot.phase).toBe('error');

    const shutdown = test.controller.shutdown();
    await vi.advanceTimersByTimeAsync(1_000);
    await expect(shutdown).resolves.toBeUndefined();
  });

  it('shows activation feedback without entering the serialized foreground-app path', async () => {
    const test = fixture({ getFrontApp: () => new Promise(() => undefined) });

    test.notify(activation('down'));
    await vi.waitFor(() => expect(test.spies.showWidget).toHaveBeenCalledOnce());
    expect(test.spies.sound).toHaveBeenCalledOnce();
    expect(test.spies.showWidget).toHaveBeenCalledWith('default', null);
    expect(test.spies.getFrontApp).not.toHaveBeenCalled();
    test.controller.cancel();
    await test.controller.shutdown();
  });

  it('starts the microphone before helper confirmation and releases the Quick model lease', async () => {
    const helperEnable = deferred<{ active: boolean }>();
    const release = vi.fn();
    const test = fixture({
      setSessionCapture: (active) =>
        active ? helperEnable.promise : Promise.resolve({ active: false }),
      acquireModelUse: () => Promise.resolve({ status: { state: 'ready' }, release }),
    });

    test.notify(activation('down'));
    await Promise.resolve();
    expect(test.spies.startDictation).toHaveBeenCalledOnce();
    expect(test.spies.sound).toHaveBeenCalledOnce();
    helperEnable.resolve({ active: true });
    await settle();

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

  it('does not start helper reset when soft-timed-out capture disable fails after shutdown begins', async () => {
    vi.useFakeTimers();
    const disable = deferred<{ active: boolean }>();
    const test = fixture({
      setSessionCapture: (active) => (active ? Promise.resolve({ active: true }) : disable.promise),
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(0);
    test.controller.cancel();
    await vi.advanceTimersByTimeAsync(1_001);

    const shutdown = test.controller.shutdown();
    expect(test.controller.shutdown()).toBe(shutdown);
    disable.reject(new Error('late disable failure'));
    await vi.advanceTimersByTimeAsync(1_001);

    await expect(shutdown).resolves.toBeUndefined();
    expect(test.spies.resetSessionCapture).not.toHaveBeenCalled();
  });

  it('cleans a resolved capture when cancellation wins before startup continuation claims it', async () => {
    const startup = deferred<{ captureId: string; activeMicrophoneId: string }>();
    const test = fixture({ startDictation: () => startup.promise });
    test.notify(activation('down'));
    await vi.waitFor(() => expect(test.spies.startDictation).toHaveBeenCalledOnce());
    void startup.promise.then(() => test.controller.cancel());

    startup.resolve({ captureId: 'same-turn-capture', activeMicrophoneId: 'default' });

    await vi.waitFor(() =>
      expect(test.spies.stopDictation).toHaveBeenCalledWith('same-turn-capture'),
    );
    expect(test.controller.snapshot.phase).toBe('cancelled');
    await test.controller.shutdown();
  });

  it('shutdown races a hung capture startup and cleans up a late capture after arming feedback', async () => {
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
    expect(test.spies.showWidget).toHaveBeenCalledOnce();
    if (startupControl.resolve === null) throw new Error('Capture startup was not invoked');
    startupControl.resolve({ captureId: 'late-capture', activeMicrophoneId: 'default' });
    await vi.waitFor(() => expect(test.spies.stopDictation).toHaveBeenCalledWith('late-capture'));
    expect(test.spies.showWidget).toHaveBeenCalledOnce();
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
      const insertionSignal = test.spies.insert.mock.calls[0]?.[1];
      if (committed) onCommitted?.();
      await vi.advanceTimersByTimeAsync(5_000);
      expect(insertionSignal?.aborted).toBe(true);
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

  it('keeps teardown single-flight and waits for delayed stream cancellation', async () => {
    vi.useFakeTimers();
    const cancellation = deferred<undefined>();
    const cancel = vi.fn(() => cancellation.promise);
    const test = fixture({
      startSession: () =>
        Promise.resolve({
          id: 'shutdown-stream',
          push: () => Promise.resolve(),
          finish: () =>
            Promise.resolve({
              text: 'unused',
              modelId: 'Xenova/whisper-small' as const,
              durationMs: 1,
              pipeline: { loadCount: 1, reused: true, loadDurationMs: 0 },
            }),
          cancel,
        }),
    });
    test.notify(activation('down'));
    await vi.advanceTimersByTimeAsync(600);
    test.frame();
    await vi.advanceTimersByTimeAsync(0);
    expect(test.spies.startSession).toHaveBeenCalledOnce();

    let shutdownSettled = false;
    const shutdown = test.controller.shutdown().then(() => {
      shutdownSettled = true;
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(cancel).toHaveBeenCalledOnce();
    expect(shutdownSettled).toBe(false);

    cancellation.resolve(undefined);
    await vi.advanceTimersByTimeAsync(0);
    await shutdown;
    expect(shutdownSettled).toBe(true);
    expect(cancel).toHaveBeenCalledOnce();
    const stopCalls = test.spies.stopDictation.mock.calls.length;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(test.spies.stopDictation).toHaveBeenCalledTimes(stopCalls);
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

  it('synchronizes full shortcut chords through the profile API', async () => {
    const test = fixture();
    const shortcut: Shortcut = {
      modifiers: { ctrl: true, alt: false, shift: false, meta: true },
      keys: ['Q', 'P'],
    };
    const result = await test.controller.updateProfile('general', { shortcut });
    expect(result.dictationProfiles.find((profile) => profile.id === 'general')?.shortcut).toEqual(
      shortcut,
    );
    expect(test.spies.configureActivation).toHaveBeenLastCalledWith(
      true,
      builtInBindings(shortcut),
    );
  });
});
