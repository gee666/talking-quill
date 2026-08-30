import { describe, expect, it, vi } from 'vitest';
import type { WhisperModelId } from '../../app/src/shared/schemas/model-manifest';
import type { ModelProgress, ModelStatus } from '../../app/src/shared/schemas/transcription';
import type { AppStateService } from '../../app/src/main/app/app-state-service';
import { ModelRuntimeCoordinator } from '../../app/src/main/app/model-runtime-coordinator';
import type { EchoSessionController } from '../../app/src/main/echo/echo-session-controller';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import type { ModelManager, WhisperWorkerClient } from '../../app/src/main/transcription';
import type { WelcomeService } from '../../app/src/main/welcome/welcome-service';

const MODEL_ID = 'Xenova/whisper-small' as WhisperModelId;

function modelStatus(
  state: ModelStatus['state'],
  downloadedBytes = state === 'ready' ? 10 : 0,
): ModelStatus {
  return {
    modelId: MODEL_ID,
    state,
    downloadedBytes,
    totalBytes: 10,
    detail: null,
    repairable: false,
  };
}

describe('model runtime coordinator', () => {
  it('publishes progress and synchronizes bound readiness targets', async () => {
    const callbacks: { progress?: (progress: ModelProgress) => void } = {};
    let settingsValue = {
      transcription: { modelId: MODEL_ID },
      welcome: { modelEvidence: {} },
    };
    const settings = {
      get: () => settingsValue,
      subscribe: vi.fn(() => () => undefined),
    } as unknown as SettingsStore;
    const models = {
      setBeforeMutation: vi.fn(),
      setAfterInstallValidation: vi.fn(),
      subscribe: vi.fn((listener: (progress: ModelProgress) => void) => {
        callbacks.progress = listener;
        return vi.fn();
      }),
      status: vi.fn(() => Promise.resolve(modelStatus('ready'))),
      manifestRevision: vi.fn(() => 'revision-1'),
    } as unknown as ModelManager;
    const send = vi.fn();
    const events = { send } as unknown as IpcEventEmitter;
    const whisper = {
      unload: vi.fn(() => Promise.resolve()),
      checkWorkerModel: vi.fn(() => Promise.resolve()),
    } as unknown as WhisperWorkerClient;
    const setModelReady = vi.fn();
    const readinessChanged = vi.fn();
    const invalidateModelSelection = vi.fn(() => Promise.resolve());
    const state = { setModelReady } as unknown as AppStateService;
    const echo = { readinessChanged } as unknown as EchoSessionController;
    const welcome = { invalidateModelSelection } as unknown as WelcomeService;
    const coordinator = new ModelRuntimeCoordinator({ settings, events, models, whisper });

    const removeProgress = coordinator.subscribeProgress();
    setModelReady((await coordinator.bindState(state)).state === 'ready');
    const unbindEcho = coordinator.bindEcho(echo);
    const unbindWelcome = coordinator.bindWelcome(welcome);
    const progress = {
      modelId: MODEL_ID,
      state: 'missing',
      file: null,
      total: { downloadedBytes: 0, totalBytes: 10 },
    } satisfies ModelProgress;
    const progressListener = callbacks.progress;
    if (progressListener === undefined) throw new Error('Progress listener was not installed');
    progressListener(progress);

    expect(send).toHaveBeenCalledWith('model:progress', progress);
    expect(setModelReady).toHaveBeenNthCalledWith(1, true);
    expect(setModelReady).toHaveBeenNthCalledWith(2, false);
    expect(readinessChanged).toHaveBeenCalledTimes(1);
    expect(invalidateModelSelection).toHaveBeenCalledTimes(1);

    unbindWelcome();
    unbindEcho();
    settingsValue = { ...settingsValue, welcome: { modelEvidence: {} } };
    progressListener(progress);
    expect(readinessChanged).toHaveBeenCalledTimes(1);
    expect(invalidateModelSelection).toHaveBeenCalledTimes(1);
    removeProgress();
  });

  it('repairs complete preserved model files before reporting setup missing', async () => {
    const settings = {
      get: () => ({ transcription: { modelId: MODEL_ID }, welcome: { modelEvidence: null } }),
      subscribe: vi.fn(() => () => undefined),
    } as unknown as SettingsStore;
    const status = vi
      .fn<(...arguments_: unknown[]) => Promise<ModelStatus>>()
      .mockResolvedValueOnce(modelStatus('corrupt', 10))
      .mockResolvedValueOnce(modelStatus('ready'));
    const models = {
      setBeforeMutation: vi.fn(),
      setAfterInstallValidation: vi.fn(),
      status,
      manifestRevision: vi.fn(() => 'revision-1'),
    } as unknown as ModelManager;
    const coordinator = new ModelRuntimeCoordinator({
      settings,
      events: { send: vi.fn() } as unknown as IpcEventEmitter,
      models,
      whisper: { unload: vi.fn() } as unknown as WhisperWorkerClient,
    });

    await expect(
      coordinator.bindState({ setModelReady: vi.fn() } as unknown as AppStateService),
    ).resolves.toMatchObject({ state: 'ready' });
    expect(status).toHaveBeenNthCalledWith(1, MODEL_ID);
    expect(status).toHaveBeenNthCalledWith(2, MODEL_ID, true);
  });

  it('keeps an incomplete model actionable without starting an automatic download', async () => {
    const settings = {
      get: () => ({ transcription: { modelId: MODEL_ID }, welcome: { modelEvidence: null } }),
      subscribe: vi.fn(() => () => undefined),
    } as unknown as SettingsStore;
    const status = vi.fn(() => Promise.resolve(modelStatus('missing', 4)));
    const models = {
      setBeforeMutation: vi.fn(),
      setAfterInstallValidation: vi.fn(),
      status,
      manifestRevision: vi.fn(() => 'revision-1'),
    } as unknown as ModelManager;
    const coordinator = new ModelRuntimeCoordinator({
      settings,
      events: { send: vi.fn() } as unknown as IpcEventEmitter,
      models,
      whisper: { unload: vi.fn() } as unknown as WhisperWorkerClient,
    });

    await expect(
      coordinator.bindState({ setModelReady: vi.fn() } as unknown as AppStateService),
    ).resolves.toMatchObject({ state: 'missing', downloadedBytes: 4 });
    expect(status).toHaveBeenCalledOnce();
  });

  it('clears stale readiness and reconciles complete files after the selected model changes', async () => {
    const replacement = 'Xenova/whisper-future' as WhisperModelId;
    let settingsValue = {
      transcription: { modelId: MODEL_ID },
      welcome: { modelEvidence: null },
    };
    let settingsListener: ((value: typeof settingsValue) => void) | undefined;
    const settings = {
      get: () => settingsValue,
      subscribe: vi.fn((listener: (value: typeof settingsValue) => void) => {
        settingsListener = listener;
        return () => undefined;
      }),
    } as unknown as SettingsStore;
    const replacementStatus = (state: ModelStatus['state']): ModelStatus => ({
      ...modelStatus(state, 10),
      modelId: replacement,
    });
    const status = vi
      .fn<(...arguments_: unknown[]) => Promise<ModelStatus>>()
      .mockResolvedValueOnce(modelStatus('ready'))
      .mockResolvedValueOnce(replacementStatus('corrupt'))
      .mockResolvedValueOnce(replacementStatus('ready'));
    const coordinator = new ModelRuntimeCoordinator({
      settings,
      events: { send: vi.fn() } as unknown as IpcEventEmitter,
      models: {
        setBeforeMutation: vi.fn(),
        setAfterInstallValidation: vi.fn(),
        status,
        manifestRevision: vi.fn(() => 'revision-1'),
      } as unknown as ModelManager,
      whisper: { unload: vi.fn() } as unknown as WhisperWorkerClient,
    });
    const setModelReady = vi.fn();
    const readinessChanged = vi.fn();
    await coordinator.bindState({ setModelReady } as unknown as AppStateService);
    coordinator.bindEcho({ readinessChanged } as unknown as EchoSessionController);
    coordinator.subscribeSelectedModel(false);
    if (settingsListener === undefined) throw new Error('Settings listener was not installed');

    settingsValue = { ...settingsValue, transcription: { modelId: replacement } };
    settingsListener(settingsValue);

    expect(setModelReady).toHaveBeenCalledWith(false);
    expect(readinessChanged).toHaveBeenCalledOnce();
    await vi.waitFor(() => expect(setModelReady).toHaveBeenLastCalledWith(true));
    expect(status).toHaveBeenNthCalledWith(2, replacement);
    expect(status).toHaveBeenNthCalledWith(3, replacement, true);
  });

  it('keeps readiness false when selected-model inspection fails', async () => {
    const replacement = 'Xenova/whisper-future' as WhisperModelId;
    let modelId = MODEL_ID;
    let settingsListener:
      ((value: { transcription: { modelId: WhisperModelId } }) => void) | undefined;
    const settings = {
      get: () => ({ transcription: { modelId }, welcome: { modelEvidence: null } }),
      subscribe: vi.fn((listener: NonNullable<typeof settingsListener>) => {
        settingsListener = listener;
        return () => undefined;
      }),
    } as unknown as SettingsStore;
    const status = vi
      .fn<(...arguments_: unknown[]) => Promise<ModelStatus>>()
      .mockResolvedValueOnce(modelStatus('ready'))
      .mockRejectedValueOnce(new Error('inspection failed'));
    const coordinator = new ModelRuntimeCoordinator({
      settings,
      events: { send: vi.fn() } as unknown as IpcEventEmitter,
      models: {
        setBeforeMutation: vi.fn(),
        setAfterInstallValidation: vi.fn(),
        status,
        manifestRevision: vi.fn(() => 'revision-1'),
      } as unknown as ModelManager,
      whisper: { unload: vi.fn() } as unknown as WhisperWorkerClient,
    });
    const setModelReady = vi.fn();
    await coordinator.bindState({ setModelReady } as unknown as AppStateService);
    coordinator.subscribeSelectedModel(false);
    if (settingsListener === undefined) throw new Error('Settings listener was not installed');

    modelId = replacement;
    settingsListener({ transcription: { modelId } });
    await vi.waitFor(() => expect(status).toHaveBeenCalledTimes(2));

    expect(setModelReady).toHaveBeenLastCalledWith(false);
  });

  it('invalidates worker-validated readiness before model mutation', async () => {
    const hooks: {
      beforeMutation?: (modelId: WhisperModelId) => Promise<void>;
      afterInstall?: (modelId: WhisperModelId, signal: AbortSignal) => Promise<void>;
    } = {};
    const settings = {
      get: () => ({ transcription: { modelId: MODEL_ID }, welcome: { modelEvidence: null } }),
      subscribe: vi.fn(() => () => undefined),
    } as unknown as SettingsStore;
    const status = vi.fn(() => Promise.resolve(modelStatus('ready')));
    const models = {
      setBeforeMutation: vi.fn((hook: (modelId: WhisperModelId) => Promise<void>) => {
        hooks.beforeMutation = hook;
      }),
      setAfterInstallValidation: vi.fn(
        (hook: (modelId: WhisperModelId, signal: AbortSignal) => Promise<void>) => {
          hooks.afterInstall = hook;
        },
      ),
      subscribe: vi.fn(() => () => undefined),
      status,
      manifestRevision: vi.fn(() => 'revision-1'),
    } as unknown as ModelManager;
    const unload = vi.fn(() => Promise.resolve());
    const whisper = {
      unload,
      checkWorkerModel: vi.fn(() => Promise.resolve()),
    } as unknown as WhisperWorkerClient;
    const coordinator = new ModelRuntimeCoordinator({
      settings,
      events: { send: vi.fn() } as unknown as IpcEventEmitter,
      models,
      whisper,
    });
    const { afterInstall, beforeMutation } = hooks;
    if (afterInstall === undefined || beforeMutation === undefined) {
      throw new Error('Model hooks were not installed');
    }

    await afterInstall(MODEL_ID, new AbortController().signal);
    status.mockClear();
    await expect(coordinator.selectedModelReadyForWelcome()).resolves.toBe(true);
    expect(status).toHaveBeenCalledTimes(1);

    await beforeMutation(MODEL_ID);
    expect(unload).toHaveBeenCalledWith(MODEL_ID);
    status.mockClear();
    await expect(coordinator.selectedModelReadyForWelcome()).resolves.toBe(true);
    expect(status).toHaveBeenNthCalledWith(1, MODEL_ID);
    expect(status).toHaveBeenNthCalledWith(2, MODEL_ID, true);
  });
});
