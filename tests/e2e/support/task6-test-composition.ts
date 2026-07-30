import type { HelperNotification } from '../../../app/src/shared/helper/protocol';
import {
  DEFAULT_GENERAL_PROFILE,
  DEFAULT_PROMPT_PROFILE,
} from '../../../app/src/shared/schemas/dictation-profiles';
import type { HelperReadiness } from '../../../app/src/shared/schemas/helper-readiness';
import type { TranscriptionResult } from '../../../app/src/shared/schemas/transcription';
import type { HistoryStore } from '../../../app/src/main/persistence/history-store';
import type { SettingsStore } from '../../../app/src/main/persistence/settings-store';
import type {
  EchoHelperPort,
  EchoInsertionPort,
  EchoRecordingPort,
  EchoWhisperPort,
  EchoSessionController,
} from '../../../app/src/main/echo/echo-session-controller';

const CAPTURE_ID = '00000000-0000-4000-8000-000000000066';

class DeterministicHelper implements EchoHelperPort {
  readonly readiness: HelperReadiness = {
    status: 'ready',
    reason: null,
    helperVersion: '1.0.0',
    permissions: {
      accessibility: 'not_applicable',
      inputMonitoring: 'not_applicable',
      eventPost: 'not_applicable',
    },
  };
  readonly #notifications = new Set<(notification: HelperNotification) => void>();

  subscribeNotifications(listener: (notification: HelperNotification) => void): () => void {
    this.#notifications.add(listener);
    return () => this.#notifications.delete(listener);
  }

  subscribeReadiness(listener: (readiness: HelperReadiness) => void): () => void {
    queueMicrotask(() => listener(this.readiness));
    return () => undefined;
  }

  configureActivation(
    enabled: boolean,
    bindings: Parameters<EchoHelperPort['configureActivation']>[1],
  ) {
    return Promise.resolve({ enabled, bindings });
  }

  setSessionCapture(active: boolean) {
    return Promise.resolve({ active });
  }

  resetSessionCapture(): Promise<void> {
    return Promise.resolve();
  }

  getFrontApp() {
    return Promise.resolve({
      processName: 'task6-target',
      windowTitle: 'Task 6 deterministic target',
      windowBounds: { x: 40, y: 40, width: 800, height: 600 },
    });
  }

  emit(notification: HelperNotification): void {
    for (const listener of this.#notifications) listener(notification);
  }
}

class DeterministicRecording implements EchoRecordingPort {
  callbacks: Parameters<EchoRecordingPort['startDictation']>[0] | null = null;
  starts = 0;
  stops = 0;

  startDictation(callbacks: Parameters<EchoRecordingPort['startDictation']>[0]) {
    this.callbacks = callbacks;
    this.starts += 1;
    return Promise.resolve({ captureId: CAPTURE_ID, activeMicrophoneId: 'task6-mic' });
  }

  stopDictation(): Promise<void> {
    this.callbacks = null;
    this.stops += 1;
    return Promise.resolve();
  }

  frames(rms: number, count: number): void {
    for (let index = 0; index < count; index += 1) {
      this.callbacks?.onFrame(new Float32Array(320).fill(rms), rms);
    }
  }
}

class DeterministicWhisper implements EchoWhisperPort {
  text = 'deterministic transcript';

  warmup() {
    return Promise.resolve();
  }

  transcribe(_pcm: Float32Array, options: Parameters<EchoWhisperPort['transcribe']>[1]) {
    return Promise.resolve(this.result(options.modelId));
  }

  startSession(options: Parameters<EchoWhisperPort['startSession']>[0]) {
    return Promise.resolve({
      id: 'task6-stream',
      push: () => Promise.resolve(),
      finish: () => Promise.resolve(this.result(options.modelId)),
      cancel: () => Promise.resolve(),
    });
  }

  private result(modelId: TranscriptionResult['modelId']): TranscriptionResult {
    return {
      text: this.text,
      modelId,
      durationMs: 1,
      pipeline: { loadCount: 1, reused: true, loadDurationMs: 0 },
    };
  }
}

class DeterministicInsertion implements EchoInsertionPort {
  targetText = '';
  copied = false;
  calls = 0;

  insert(text: string, signal?: AbortSignal, onCommitted?: () => void) {
    if (signal?.aborted === true) return Promise.reject(new Error('aborted'));
    this.calls += 1;
    this.targetText = text;
    if (!this.copied) onCommitted?.();
    return Promise.resolve({ inserted: !this.copied, copied: this.copied });
  }
}

export interface Task6TestDriver {
  activationComplete(heldMs?: number): void;
  activationDown(alternate?: boolean): void;
  activationUp(alternate?: boolean): void;
  key(key: 'escape' | 'enter'): void;
  frames(rms: number, count: number): void;
  setTranscript(text: string): void;
  setCopied(copied: boolean): void;
  setWelcomePrerequisites(ready: boolean): Promise<void>;
  snapshot(): unknown;
}

export function createTask6TestComposition(
  history?: HistoryStore,
  settings?: SettingsStore,
  realRecording?: EchoRecordingPort,
) {
  const helper = new DeterministicHelper();
  const recording = new DeterministicRecording();
  const whisper = new DeterministicWhisper();
  const insertion = new DeterministicInsertion();
  let controller: EchoSessionController | null = null;
  const welcome = { microphone: false, model: false };
  const requireController = () => {
    if (controller === null) throw new Error('Task 6 controller is not bound');
    return controller;
  };
  const driver: Task6TestDriver = Object.freeze({
    activationComplete: (heldMs = 100) =>
      helper.emit({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'complete',
          profileId: 'general',
          shortcut: DEFAULT_GENERAL_PROFILE.shortcut,
          heldMs,
        },
      }),
    activationDown: (alternate = false) =>
      helper.emit({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'down',
          profileId: alternate ? 'prompt' : 'general',
          shortcut: (alternate ? DEFAULT_PROMPT_PROFILE : DEFAULT_GENERAL_PROFILE).shortcut,
        },
      }),
    activationUp: (alternate = false) =>
      helper.emit({
        jsonrpc: '2.0',
        method: 'activation.event',
        params: {
          phase: 'up',
          profileId: alternate ? 'prompt' : 'general',
          shortcut: (alternate ? DEFAULT_PROMPT_PROFILE : DEFAULT_GENERAL_PROFILE).shortcut,
        },
      }),
    key: (key: 'escape' | 'enter') =>
      helper.emit({
        jsonrpc: '2.0',
        method: 'session.key',
        params: { key, phase: 'down' },
      }),
    frames: (rms: number, count: number) => recording.frames(rms, count),
    setTranscript: (text: string) => {
      whisper.text = text;
    },
    setCopied: (copied: boolean) => {
      insertion.copied = copied;
    },
    setWelcomePrerequisites: async (ready: boolean) => {
      welcome.microphone = ready;
      welcome.model = ready;
      await settings?.update({
        welcome: { microphoneTested: ready, activationTested: false },
      });
    },
    snapshot: () => ({
      session: requireController().snapshot,
      recording: {
        starts: recording.starts,
        stops: recording.stops,
        active: recording.callbacks !== null,
      },
      insertion: {
        calls: insertion.calls,
        targetText: insertion.targetText,
        copied: insertion.copied,
      },
      history: history?.list(200).items ?? [],
    }),
  });
  return {
    helper,
    recording: realRecording ?? recording,
    whisper,
    insertion,
    welcome,
    driver,
    startPackagedMedia() {
      driver.activationComplete(100);
    },
    bind(next: EchoSessionController) {
      controller = next;
    },
  };
}
