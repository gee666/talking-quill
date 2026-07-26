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

function modelStatus(state: ModelStatus['state']): ModelStatus {
  return {
    modelId: MODEL_ID,
    state,
    downloadedBytes: state === 'ready' ? 10 : 0,
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
