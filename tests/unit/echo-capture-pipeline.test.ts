import type { WindowManager } from '../../app/src/main/app/window-manager';
import type { DictationCaptureCallbacks } from '../../app/src/main/audio/recording-service';
import { EchoCapturePipeline } from '../../app/src/main/echo/echo-capture-pipeline';
import type {
  EchoRecordingPort,
  EchoWhisperPort,
  WhisperStreamingSession,
} from '../../app/src/main/echo/echo-session-ports';
import type { HelperCaptureReconciler } from '../../app/src/main/echo/helper-capture-reconciler';
import {
  IDLE_ECHO_SESSION,
  reduceEchoSession,
  type EchoSessionState,
} from '../../app/src/main/echo/session-reducer';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function fixture() {
  let state: EchoSessionState = IDLE_ECHO_SESSION;
  let abort = new AbortController();
  let generation = 0;
  const callbacks: DictationCaptureCallbacks[] = [];
  const startDictation = vi.fn<EchoRecordingPort['startDictation']>((listener) => {
    callbacks.push(listener);
    return Promise.resolve({
      captureId: `capture-${String(callbacks.length)}`,
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
    });
  });
  const stopDictation = vi.fn<EchoRecordingPort['stopDictation']>(() => Promise.resolve());
  const result = {
    text: 'local transcript',
    modelId: 'Xenova/whisper-small' as const,
    durationMs: 5,
    pipeline: { loadCount: 1, reused: false, loadDurationMs: 1 },
  };
  const transcribe = vi.fn<EchoWhisperPort['transcribe']>(() => Promise.resolve(result));
  const stream = {
    id: 'stream',
    push: vi.fn(() => Promise.resolve()),
    finish: vi.fn(() => Promise.resolve(result)),
    cancel: vi.fn(() => Promise.resolve()),
  } satisfies WhisperStreamingSession;
  const startSession = vi.fn<EchoWhisperPort['startSession']>(() => Promise.resolve(stream));
  const release = vi.fn();
  const playSound = vi.fn();
  const pipeline = new EchoCapturePipeline({
    recording: { startDictation, stopDictation },
    whisper: { transcribe, startSession },
    captureReconciler: {
      beginGeneration: () => ++generation,
      request: () => Promise.resolve(),
    } as unknown as HelperCaptureReconciler,
    windows: {
      createWidgetForActivation: () => true,
      showWidget: () => true,
      showMain: vi.fn(),
    } as unknown as WindowManager,
    getWidgetSize: () => DEFAULT_SETTINGS.app.widgetSize,
    playSound,
    getState: () => state,
    getSignal: () => abort.signal,
    abort: () => abort.abort(),
    dispatch: (event) => {
      state = reduceEchoSession(state, event).state;
    },
    acquireModelUse: () => Promise.resolve({ status: { state: 'ready' }, release }),
  });
  return {
    pipeline,
    callbacks,
    stopDictation,
    transcribe,
    startSession,
    stream,
    release,
    playSound,
    abort: () => abort.abort(),
    start: async () => {
      abort = new AbortController();
      state = {
        ...IDLE_ECHO_SESSION,
        phase: 'recordingQuick',
        dictationMode: 'quick',
        processingMode: 'raw',
      };
      pipeline.beginGeneration();
      const settings = structuredClone(DEFAULT_SETTINGS);
      settings.recording.autoSubmitOnSilence = false;
      pipeline.arm(settings);
      await pipeline.startCapture();
    },
    setPhase: (phase: EchoSessionState['phase']) => {
      const previous = state;
      state = { ...state, phase };
      pipeline.observeTransition(previous, state);
    },
  };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('EchoCapturePipeline ownership', () => {
  it('keeps PCM and model ownership separate across pipeline instances', async () => {
    const first = fixture();
    const second = fixture();
    await Promise.all([first.start(), second.start()]);
    const samples = new Float32Array([0.1, 0.2]);
    first.callbacks[0]?.onFrame(samples, 0.2);
    samples.fill(0.9);
    second.callbacks[0]?.onFrame(new Float32Array([0.3]), 0.3);
    await first.pipeline.transcribe();
    expect(first.transcribe.mock.calls[0]?.[0]).toEqual(new Float32Array([0.1, 0.2]));
    await first.pipeline.performTeardown(() => undefined);
    expect(first.release).toHaveBeenCalledOnce();
    expect(second.release).not.toHaveBeenCalled();
    expect(second.pipeline.captureId).toBe('capture-1');
    await second.pipeline.transcribe();
    expect(second.transcribe.mock.calls[0]?.[0]).toEqual(new Float32Array([0.3]));
    await second.pipeline.performTeardown(() => undefined);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('rejects stale callbacks after teardown and announces readiness once per generation', async () => {
    const test = fixture();
    await test.start();
    const old = test.callbacks[0];
    expect(test.playSound).not.toHaveBeenCalled();
    old?.onFrame(new Float32Array([0.1]), 0.1);
    old?.onFrame(new Float32Array([0.2]), 0.2);
    expect(test.playSound).toHaveBeenCalledOnce();
    await test.pipeline.performTeardown(() => undefined);
    await test.start();
    old?.onFrame(new Float32Array([0.9]), 0.9);
    old?.onUnexpectedStop('device-unavailable');
    test.callbacks[1]?.onFrame(new Float32Array([0.3]), 0.3);
    await test.pipeline.transcribe();
    expect(test.transcribe.mock.calls[0]?.[0]).toEqual(new Float32Array([0.3]));
    expect(test.playSound).toHaveBeenCalledTimes(2);
    expect(test.pipeline.generation).toBe(2);
    await test.pipeline.performTeardown(() => undefined);
    expect(vi.getTimerCount()).toBe(0);
  });

  it('retains final PCM while stop drains and ignores frames after its acknowledgement', async () => {
    const test = fixture();
    await test.start();
    test.callbacks[0]?.onFrame(new Float32Array([0.1]), 0.1);
    test.setPhase('transcribing');
    const stopped = deferred<undefined>();
    test.stopDictation.mockReturnValueOnce(stopped.promise);
    const stopping = test.pipeline.stopCapture();
    test.callbacks[0]?.onFrame(new Float32Array([0.2]), 0.2);
    stopped.resolve(undefined);
    await stopping;
    test.callbacks[0]?.onFrame(new Float32Array([0.9]), 0.9);
    await test.pipeline.transcribe();
    expect(test.transcribe.mock.calls[0]?.[0]).toEqual(new Float32Array([0.1, 0.2]));
    await test.pipeline.performTeardown(() => undefined);
    expect(test.stopDictation).toHaveBeenCalledOnce();
    expect(vi.getTimerCount()).toBe(0);
  });

  it('cancels a late stream opening after its owner aborts', async () => {
    const test = fixture();
    await test.start();
    const opening = deferred<WhisperStreamingSession>();
    test.startSession.mockReturnValueOnce(opening.promise);
    const extended = test.pipeline.beginExtendedTranscription();
    const rejected = expect(extended).rejects.toMatchObject({ name: 'AbortError' });
    test.abort();
    await rejected;
    opening.resolve(test.stream);
    await opening.promise;
    await test.pipeline.performTeardown(() => undefined);
    expect(test.stream.cancel).toHaveBeenCalledOnce();
    expect(test.stream.finish).not.toHaveBeenCalled();
    expect(test.release).toHaveBeenCalledOnce();
    expect(test.pipeline.captureId).toBeNull();
    expect(vi.getTimerCount()).toBe(0);
  });
});
