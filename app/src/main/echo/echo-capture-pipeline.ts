import {
  CAPTURE_CANCEL_TIMEOUT_MS,
  PCM_FRAME_DURATION_MS,
  PCM_SAMPLE_RATE,
  SESSION_CAP_MS,
} from '../../shared/constants/audio';
import {
  ECHO_HOLD_THRESHOLD_MS,
  ECHO_LEVEL_EVENT_INTERVAL_MS,
} from '../../shared/constants/echo-session';
import { WHISPER_MAX_PUSH_SAMPLES } from '../../shared/constants/whisper';
import type { HelperSessionCaptureMode } from '../../shared/helper/protocol';
import { TranscriptionResultSchema } from '../../shared/schemas/transcription';
import type { Settings } from '../../shared/schemas/settings';
import { SilencePolicy } from '../audio/silence-policy';
import type { WindowManager } from '../app/window-manager';
import {
  abortOperationError,
  normalizeOperationError,
  operationError,
  raceWithAbort,
  withSoftTimeout,
} from './echo-operation';
import type {
  EchoModelUseGrant,
  EchoRecordingPort,
  EchoWhisperPort,
  WhisperStreamingSession,
} from './echo-session-ports';
import type { HelperCaptureReconciler } from './helper-capture-reconciler';
import { concatChunks, discardChunkPrefix, sliceChunks } from './pcm-buffer';
import type { EchoSessionEvent, EchoSessionState } from './session-reducer';
import { helperCaptureModeForPhase, isCapturePhase, isTerminalPhase } from './session-phase';

const MAX_EXTENDED_BUFFERED_SAMPLES = WHISPER_MAX_PUSH_SAMPLES * 2;
const MODEL_USE_SETTLE_TIMEOUT_MS = 1_000;
const STREAM_CANCEL_TIMEOUT_MS = 1_000;
const FIRST_AUDIO_TIMEOUT_MS = 1_000;

interface CaptureSessionOwner {
  readonly generation: number;
  readonly signal: AbortSignal;
  readonly transcription: Readonly<Settings['transcription']>;
  readonly includeSystemAudio: boolean;
  active: boolean;
  captureOpening: boolean;
}

export class EchoCapturePipeline {
  readonly #recording: EchoRecordingPort;
  readonly #whisper: EchoWhisperPort;
  readonly #captureReconciler: HelperCaptureReconciler;
  readonly #windows: WindowManager;
  readonly #getWidgetSize: () => Settings['app']['widgetSize'];
  readonly #playSound: () => void;
  readonly #getState: () => EchoSessionState;
  readonly #getSignal: () => AbortSignal;
  readonly #abort: () => void;
  readonly #dispatch: (event: EchoSessionEvent) => void;
  readonly #acquireModelUse: (
    modelId: Settings['transcription']['modelId'],
    signal: AbortSignal,
  ) => Promise<EchoModelUseGrant>;
  #generation = 0;
  #sessionOwner: CaptureSessionOwner | null = null;
  #captureId: string | null = null;
  #captureStopCompleted = false;
  #captureStopping = false;
  #pcmChunks: Float32Array[] = [];
  #totalSamples = 0;
  #streamedSamples = 0;
  #discardedSamples = 0;
  #streamPushPending = false;
  #stream: WhisperStreamingSession | null = null;
  #streamOpening: Promise<WhisperStreamingSession> | null = null;
  #streamTail: Promise<void> = Promise.resolve();
  #streamFailure: unknown = null;
  #modelUse: EchoModelUseGrant | null = null;
  #modelUseOpening: Promise<void> = Promise.resolve();
  #warmupOpening: Promise<void> = Promise.resolve();
  #holdTimer: ReturnType<typeof setTimeout> | null = null;
  #capTimer: ReturnType<typeof setTimeout> | null = null;
  #audioStartTimer: ReturnType<typeof setTimeout> | null = null;
  #silence: SilencePolicy | null = null;
  #pendingSilenceSubmit = false;
  #lastLevelAt = 0;

  constructor(options: {
    readonly recording: EchoRecordingPort;
    readonly whisper: EchoWhisperPort;
    readonly captureReconciler: HelperCaptureReconciler;
    readonly windows: WindowManager;
    readonly getWidgetSize: () => Settings['app']['widgetSize'];
    readonly playSound: () => void;
    readonly getState: () => EchoSessionState;
    readonly getSignal: () => AbortSignal;
    readonly abort: () => void;
    readonly dispatch: (event: EchoSessionEvent) => void;
    readonly acquireModelUse: (
      modelId: Settings['transcription']['modelId'],
      signal: AbortSignal,
    ) => Promise<EchoModelUseGrant>;
  }) {
    this.#recording = options.recording;
    this.#whisper = options.whisper;
    this.#captureReconciler = options.captureReconciler;
    this.#windows = options.windows;
    this.#getWidgetSize = options.getWidgetSize;
    this.#playSound = options.playSound;
    this.#getState = options.getState;
    this.#getSignal = options.getSignal;
    this.#abort = options.abort;
    this.#dispatch = options.dispatch;
    this.#acquireModelUse = options.acquireModelUse;
  }

  get generation(): number {
    return this.#generation;
  }

  get captureId(): string | null {
    return this.#captureId;
  }

  beginGeneration(): number {
    if (this.#sessionOwner !== null) this.#sessionOwner.active = false;
    this.#sessionOwner = null;
    this.#generation = this.#captureReconciler.beginGeneration();
    this.#captureStopCompleted = false;
    this.#captureStopping = false;
    this.#pcmChunks = [];
    this.#totalSamples = 0;
    this.#streamedSamples = 0;
    this.#discardedSamples = 0;
    this.#streamPushPending = false;
    this.#stream = null;
    this.#streamOpening = null;
    this.#streamTail = Promise.resolve();
    this.#streamFailure = null;
    this.#modelUse = null;
    this.#warmupOpening = Promise.resolve();
    return this.#generation;
  }

  arm(settings: Readonly<Settings>): void {
    const owner: CaptureSessionOwner = {
      generation: this.#generation,
      signal: this.#getSignal(),
      transcription: settings.transcription,
      includeSystemAudio: settings.recording.includeSystemAudio,
      active: true,
      captureOpening: false,
    };
    this.#sessionOwner = owner;
    const modelId = owner.transcription.modelId;
    this.#modelUseOpening = this.#acquireModelUse(modelId, owner.signal).then((grant) => {
      if (!this.#isActive(owner) || !isCapturePhase(this.#getState().phase)) {
        grant.release();
        return;
      }
      if (grant.status.state !== 'ready') {
        grant.release();
        throw new Error('The selected transcription model is unavailable');
      }
      this.#modelUse = grant;
    });
    // The start-capture effect observes this promise. Attaching a rejection handler now keeps
    // synchronous model-loss races from becoming process-level unhandled rejections.
    void this.#modelUseOpening.catch(() => undefined);
    // Warm the inference pipeline independently of capture readiness. This begins at shortcut-down
    // and overlaps the user's speech without delaying the widget, cue, helper, or microphone.
    this.#warmupOpening = this.#whisper.warmup?.(modelId, owner.signal) ?? Promise.resolve();
    void this.#warmupOpening.catch(() => undefined);
    this.#pendingSilenceSubmit = false;
    this.#silence = settings.recording.autoSubmitOnSilence
      ? new SilencePolicy({
          mode: 'quick',
          preset: settings.recording.silencePreset,
        })
      : null;
    this.#holdTimer = setTimeout(() => {
      if (this.#isActive(owner)) this.#dispatch({ type: 'hold-elapsed', now: Date.now() });
    }, ECHO_HOLD_THRESHOLD_MS);
    this.#holdTimer.unref();
    this.#capTimer = setTimeout(() => {
      if (this.#isActive(owner)) this.#dispatch({ type: 'submit', source: 'duration-cap' });
    }, SESSION_CAP_MS.extended);
    this.#capTimer.unref();
  }

  observeTransition(previous: EchoSessionState, next: EchoSessionState): void {
    const owner = this.#sessionOwner;
    if (previous.phase === 'arming' && next.phase === 'recordingQuick') {
      this.#clearHoldTimer();
      this.#replaceCapTimer(owner, SESSION_CAP_MS.quick - next.elapsedMs);
      if (this.#pendingSilenceSubmit && owner !== null) {
        queueMicrotask(() => {
          if (this.#isActive(owner)) this.#dispatch({ type: 'submit', source: 'silence' });
        });
      }
    }
    if (next.phase === 'recordingExtended' && previous.phase !== 'recordingExtended') {
      this.#clearHoldTimer();
      this.#silence = null;
    }
    if (next.phase === 'transcribing' || next.phase === 'cancelled' || next.phase === 'error') {
      this.#clearRecordingTimers();
    }
  }

  async startCapture(): Promise<void> {
    const owner = this.#requireActiveOwner();
    if (!isCapturePhase(this.#getState().phase)) return;
    // The widget renderer is preloaded. Show its truthful arming state before any device, model,
    // or helper round trip so the global shortcut always receives immediate visual feedback.
    if (!this.#windows.showWidget(this.#getWidgetSize(), null)) {
      this.#windows.showMain();
      throw new Error('Dictation could not start because its status widget is unavailable.');
    }
    // A shortcut acknowledgement must not wait for model loading, helper IPC, or microphone
    // startup. Beep first so the user can speak immediately and so the cue is not recorded once
    // capture begins opening below.
    this.#playSound();

    // Model readiness, native key capture, and microphone startup are independent. Open all three
    // concurrently so none of their latencies are added together and opening speech is retained.
    const helperCaptureOpening = this.#captureReconciler.request('recording', owner.generation);
    owner.captureOpening = true;
    let capturePromise: ReturnType<EchoRecordingPort['startDictation']>;
    try {
      capturePromise = this.#recording.startDictation(
        {
          onFrame: (samples, rms) => this.#onFrame(owner, samples, rms),
          onUnexpectedStop: (reason) => {
            if (this.#isActive(owner)) {
              this.#dispatch({
                type: 'fail',
                message:
                  reason === 'device-unavailable'
                    ? 'The microphone stopped unexpectedly.'
                    : reason === 'system-audio-unavailable'
                      ? 'System audio stopped unexpectedly.'
                      : 'Audio capture stopped unexpectedly.',
              });
            }
          },
        },
        { includeSystemAudio: owner.includeSystemAudio },
      );
    } catch (error: unknown) {
      owner.captureOpening = false;
      throw error;
    }
    void capturePromise.then(
      async (capture) => {
        owner.captureOpening = false;
        if (!this.#captureStillCurrent(owner)) {
          await this.#recording.stopDictation(capture.captureId).catch(() => undefined);
          return;
        }
        this.#captureId = capture.captureId;
        if (!this.#getState().audioReady) this.#armFirstAudioTimer(owner);
      },
      () => {
        owner.captureOpening = false;
      },
    );

    await raceWithAbort(
      Promise.all([this.#modelUseOpening, helperCaptureOpening, capturePromise]),
      owner.signal,
    );
    if (!this.#captureStillCurrent(owner)) return;
    this.#dispatch({ type: 'capture-started' });
  }

  async beginExtendedTranscription(): Promise<void> {
    const owner = this.#requireActiveOwner();
    await this.#ensureExtendedStream(owner);
    if (this.#isActive(owner)) this.#queueExtendedAudio(owner, false);
  }

  async stopCapture(
    owner = this.#sessionOwner,
    helperMode: HelperSessionCaptureMode = helperCaptureModeForPhase(this.#getState().phase),
  ): Promise<void> {
    const ownsSession = owner !== null && this.#ownsSession(owner);
    const generation = owner?.generation ?? this.#generation;
    const captureId = ownsSession ? this.#captureId : null;
    const shouldCancelOpening = ownsSession && captureId === null && owner.captureOpening;
    const shouldStopRecording =
      !this.#captureStopCompleted && (captureId !== null || shouldCancelOpening);
    if (ownsSession) this.#captureStopping = captureId !== null && shouldStopRecording;
    const recordingStop = shouldStopRecording
      ? withSoftTimeout(
          operationError(
            () => this.#recording.stopDictation(captureId ?? undefined),
            'Microphone stop failed',
          ),
          CAPTURE_CANCEL_TIMEOUT_MS,
          new Error('Microphone stop timed out'),
        )
      : Promise.resolve(null);
    // Change native key capture immediately rather than waiting for microphone teardown. Both
    // boundaries are independently bounded because custom ports need not honor production limits.
    const helperStop = withSoftTimeout(
      operationError(
        () => this.#captureReconciler.request(helperMode, generation),
        'Helper capture mode change failed',
      ),
      CAPTURE_CANCEL_TIMEOUT_MS,
      new Error('Helper capture mode change timed out'),
    );
    let errors: readonly Error[];
    try {
      const [recordingError, helperError] = await Promise.all([recordingStop, helperStop]);
      if (ownsSession && this.#ownsSession(owner) && shouldStopRecording) {
        this.#captureStopCompleted = recordingError === null;
      }
      errors = [recordingError, helperError].filter((error): error is Error => error !== null);
    } finally {
      if (ownsSession && this.#ownsSession(owner)) this.#captureStopping = false;
    }
    const firstError = errors[0];
    if (errors.length === 1 && firstError !== undefined) throw firstError;
    if (errors.length > 1) throw new AggregateError(errors, 'Capture stop failed');
  }

  async transcribe(): Promise<string> {
    const owner = this.#requireActiveOwner();
    const settings = owner.transcription;
    if (this.#getState().dictationMode === 'extended') {
      await this.#ensureExtendedStream(owner);
      this.#assertActive(owner);
      while (this.#streamedSamples < this.#totalSamples || this.#streamPushPending) {
        this.#queueExtendedAudio(owner, true);
        const tail = this.#streamTail;
        await tail;
        this.#assertActive(owner);
        if (this.#streamFailure !== null) break;
      }
      if (this.#streamFailure !== null) {
        throw normalizeOperationError(this.#streamFailure, 'Stream failed');
      }
      const stream = this.#stream;
      if (stream === null) throw new Error('Transcription stream was unavailable');
      const result = await stream.finish(owner.signal);
      this.#assertActive(owner);
      return TranscriptionResultSchema.parse(result).text;
    }
    if (this.#totalSamples === 0) throw new Error('No audio was captured');
    const result = await this.#whisper.transcribe(
      concatChunks(this.#pcmChunks, this.#totalSamples),
      {
        modelId: settings.modelId,
        sampleRate: PCM_SAMPLE_RATE,
        language: settings.language,
      },
      owner.signal,
    );
    this.#assertActive(owner);
    return TranscriptionResultSchema.parse(result).text;
  }

  async performTeardown(afterTimersCleared: () => void): Promise<void> {
    const owner = this.#sessionOwner;
    const opening = this.#streamOpening;
    const ownedStream = this.#stream;
    const streamTail = this.#streamTail;
    const modelUseOpening = this.#modelUseOpening;
    const warmupOpening = this.#warmupOpening;
    const modelUse = this.#modelUse;
    if (owner !== null) owner.active = false;
    this.#clearRecordingTimers();
    afterTimersCleared();
    let captureError: unknown = null;
    try {
      await this.stopCapture(owner);
    } catch (error: unknown) {
      captureError = error;
    } finally {
      const stream =
        ownedStream ??
        (opening === null
          ? null
          : await withSoftTimeout(
              opening.catch(() => null),
              STREAM_CANCEL_TIMEOUT_MS,
              null,
            ));
      const streamCancellation =
        stream !== null && this.#getState().phase !== 'completed'
          ? stream.cancel().catch(() => undefined)
          : Promise.resolve();
      await Promise.all([
        withSoftTimeout(
          streamTail.catch(() => undefined),
          STREAM_CANCEL_TIMEOUT_MS,
          undefined,
        ),
        withSoftTimeout(streamCancellation, STREAM_CANCEL_TIMEOUT_MS, undefined),
      ]);
      await Promise.all([
        withSoftTimeout(
          modelUseOpening.catch(() => undefined),
          MODEL_USE_SETTLE_TIMEOUT_MS,
          undefined,
        ),
        withSoftTimeout(
          warmupOpening.catch(() => undefined),
          MODEL_USE_SETTLE_TIMEOUT_MS,
          undefined,
        ),
      ]);
      modelUse?.release();
      if (this.#ownsSession(owner)) {
        this.#sessionOwner = null;
        this.#captureId = null;
        this.#captureStopCompleted = false;
        this.#captureStopping = false;
        this.#stream = null;
        this.#streamOpening = null;
        this.#modelUse = null;
        this.#modelUseOpening = Promise.resolve();
        this.#warmupOpening = Promise.resolve();
        this.#pcmChunks = [];
        this.#totalSamples = 0;
        this.#streamedSamples = 0;
        this.#discardedSamples = 0;
        this.#streamPushPending = false;
        this.#streamFailure = null;
        this.#silence = null;
      }
    }
    if (captureError !== null) {
      throw normalizeOperationError(captureError, 'Capture teardown failed');
    }
  }

  #onFrame(owner: CaptureSessionOwner, samples: Float32Array, rms: number): void {
    if (!this.#isActive(owner)) return;
    const state = this.#getState();
    const draining = state.phase === 'transcribing' && this.#captureStopping;
    if (!isCapturePhase(state.phase) && !draining) return;
    const copy = Float32Array.from(samples);
    this.#pcmChunks.push(copy);
    this.#totalSamples += copy.length;
    if (this.#audioStartTimer !== null) {
      clearTimeout(this.#audioStartTimer);
      this.#audioStartTimer = null;
    }
    if (!this.#getState().audioReady) this.#dispatch({ type: 'audio-started' });
    const elapsedMs = Math.round((this.#totalSamples / PCM_SAMPLE_RATE) * 1_000);
    const now = Date.now();
    if (now - this.#lastLevelAt >= ECHO_LEVEL_EVENT_INTERVAL_MS) {
      this.#lastLevelAt = now;
      this.#dispatch({ type: 'level', rms, elapsedMs });
    }
    if (!draining && this.#silence !== null) {
      const decision = this.#silence.observe({
        rms,
        durationMs: (copy.length / PCM_SAMPLE_RATE) * 1_000 || PCM_FRAME_DURATION_MS,
        elapsedMs,
      });
      if (decision !== null) {
        if (this.#getState().phase === 'recordingQuick') {
          this.#dispatch({
            type: 'submit',
            source: decision === 'duration-cap' ? 'duration-cap' : 'silence',
          });
        } else this.#pendingSilenceSubmit = true;
      }
    }
    if (this.#getState().phase === 'recordingExtended') {
      if (this.#totalSamples - this.#discardedSamples > MAX_EXTENDED_BUFFERED_SAMPLES) {
        this.#dispatch({
          type: 'fail',
          message: 'Transcription could not keep up with captured audio.',
        });
        return;
      }
      this.#queueExtendedAudio(owner, false);
    }
  }

  async #ensureExtendedStream(owner: CaptureSessionOwner): Promise<WhisperStreamingSession> {
    this.#assertActive(owner);
    if (this.#stream !== null) return this.#stream;
    if (this.#streamOpening !== null) return raceWithAbort(this.#streamOpening, owner.signal);
    const settings = owner.transcription;
    const startup = this.#whisper.startSession(
      {
        modelId: settings.modelId,
        sampleRate: PCM_SAMPLE_RATE,
        language: settings.language,
      },
      owner.signal,
    );
    const opening = startup.then((stream) => {
      if (!this.#isActive(owner)) {
        void stream.cancel().catch(() => undefined);
        throw abortOperationError();
      }
      // Claim the stream before waking abort-bound waiters. If abort wins that race, teardown can
      // still find and cancel this stream; a later non-cooperative startup cancels itself above.
      this.#stream = stream;
      return stream;
    });
    // Teardown may abandon this promise after aborting it. Keep a late worker rejection observed.
    void opening.catch(() => undefined);
    this.#streamOpening = opening;
    try {
      return await raceWithAbort(opening, owner.signal);
    } finally {
      if (this.#ownsSession(owner) && this.#streamOpening === opening) {
        this.#streamOpening = null;
      }
    }
  }

  #queueExtendedAudio(owner: CaptureSessionOwner, force: boolean): void {
    if (!this.#isActive(owner) || this.#streamPushPending) return;
    const available = this.#totalSamples - this.#streamedSamples;
    if (available <= 0 || (!force && available < WHISPER_MAX_PUSH_SAMPLES)) return;
    const start = this.#streamedSamples;
    const count = force ? available : Math.min(available, WHISPER_MAX_PUSH_SAMPLES);
    const relativeStart = start - this.#discardedSamples;
    if (relativeStart < 0) throw new Error('Extended transcription buffer accounting failed');
    const pcm = sliceChunks(this.#pcmChunks, relativeStart, count);
    this.#streamedSamples += count;
    this.#streamPushPending = true;
    const precedingTail = this.#streamTail;
    const push = precedingTail.then(async () => {
      this.#assertActive(owner);
      if (this.#streamFailure !== null) {
        throw normalizeOperationError(this.#streamFailure, 'Transcription stream failed');
      }
      const stream = await this.#ensureExtendedStream(owner);
      this.#assertActive(owner);
      await stream.push(pcm, owner.signal);
      this.#assertActive(owner);
      if (this.#totalSamples - this.#discardedSamples >= count) {
        this.#pcmChunks = discardChunkPrefix(this.#pcmChunks, count);
        this.#discardedSamples += count;
      }
    });
    // Observe every background rejection immediately, but retain the first failure so submit can
    // never finish a stream after silently dropping audio. Only one push is retained at a time;
    // successful audio is discarded immediately to bound a 30-minute Extended session.
    this.#streamTail = push
      .catch((error: unknown) => {
        if (!this.#isActive(owner)) return;
        this.#streamFailure ??= error;
        if (!isTerminalPhase(this.#getState().phase)) {
          this.#dispatch({ type: 'fail', message: publicSessionError(error) });
        }
      })
      .finally(() => {
        if (!this.#isActive(owner)) return;
        this.#streamPushPending = false;
        if (this.#getState().phase === 'recordingExtended') {
          this.#queueExtendedAudio(owner, false);
        }
      });
  }

  #requireActiveOwner(): CaptureSessionOwner {
    const owner = this.#sessionOwner;
    if (owner === null || !this.#isActive(owner)) throw abortOperationError();
    return owner;
  }

  #assertActive(owner: CaptureSessionOwner): void {
    if (!this.#isActive(owner)) throw abortOperationError();
  }

  #ownsSession(owner: CaptureSessionOwner | null): boolean {
    return (
      this.#sessionOwner === owner && (owner === null || owner.generation === this.#generation)
    );
  }

  #isActive(owner: CaptureSessionOwner): boolean {
    return this.#ownsSession(owner) && owner.active && !owner.signal.aborted;
  }

  #captureStillCurrent(owner: CaptureSessionOwner): boolean {
    return this.#isActive(owner) && isCapturePhase(this.#getState().phase);
  }

  #armFirstAudioTimer(owner: CaptureSessionOwner): void {
    if (this.#audioStartTimer !== null) clearTimeout(this.#audioStartTimer);
    this.#audioStartTimer = setTimeout(() => {
      if (!this.#isActive(owner)) return;
      this.#audioStartTimer = null;
      if (this.#getState().audioReady || !isCapturePhase(this.#getState().phase)) return;
      this.#abort();
      this.#dispatch({ type: 'fail', message: 'The microphone did not provide audio.' });
    }, FIRST_AUDIO_TIMEOUT_MS);
    this.#audioStartTimer.unref();
  }

  #replaceCapTimer(owner: CaptureSessionOwner | null, milliseconds: number): void {
    if (this.#capTimer !== null) clearTimeout(this.#capTimer);
    this.#capTimer = setTimeout(
      () => {
        if (owner !== null && this.#isActive(owner)) {
          this.#dispatch({ type: 'submit', source: 'duration-cap' });
        }
      },
      Math.max(1, milliseconds),
    );
    this.#capTimer.unref();
  }

  #clearHoldTimer(): void {
    if (this.#holdTimer !== null) clearTimeout(this.#holdTimer);
    this.#holdTimer = null;
  }

  #clearRecordingTimers(): void {
    this.#clearHoldTimer();
    if (this.#capTimer !== null) clearTimeout(this.#capTimer);
    this.#capTimer = null;
    if (this.#audioStartTimer !== null) clearTimeout(this.#audioStartTimer);
    this.#audioStartTimer = null;
  }
}

function publicSessionError(error: unknown): string {
  void error;
  return 'Dictation could not be completed.';
}
