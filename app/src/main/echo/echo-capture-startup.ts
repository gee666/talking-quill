import { announceCaptureReady } from './echo-capture-readiness';
import { SESSION_CAP_MS } from '../../shared/constants/audio';
import { ECHO_HOLD_THRESHOLD_MS } from '../../shared/constants/echo-session';
import type { Settings } from '../../shared/schemas/settings';
import { SilencePolicy } from '../audio/silence-policy';
import { onFrame } from './echo-capture-audio';
import {
  captureStillCurrent,
  isActive,
  ownsSession,
  requireActiveOwner,
  type CaptureSessionOwner,
  type EchoCaptureContext,
} from './echo-capture-context';
import { raceWithAbort } from './echo-operation';
import type { EchoRecordingPort } from './echo-session-ports';
import { isCapturePhase } from './session-phase';
import type { EchoSessionState } from './session-reducer';

const FIRST_AUDIO_TIMEOUT_MS = 1_000;

export function beginGeneration(context: EchoCaptureContext): number {
  if (context.sessionOwner !== null) context.sessionOwner.active = false;
  context.sessionOwner = null;
  context.generation = context.captureReconciler.beginGeneration();
  context.captureStopCompleted = false;
  context.nativeCaptureLost = false;
  context.readyCuePlayed = false;
  context.captureStopping = false;
  context.pcmChunks = [];
  context.totalSamples = 0;
  context.streamedSamples = 0;
  context.discardedSamples = 0;
  context.streamPushPending = false;
  context.stream = null;
  context.streamOpening = null;
  context.streamTail = Promise.resolve();
  context.streamFailure = null;
  context.modelUse = null;
  context.warmupOpening = Promise.resolve();
  return context.generation;
}

export function arm(context: EchoCaptureContext, settings: Readonly<Settings>): void {
  const owner: CaptureSessionOwner = {
    generation: context.generation,
    signal: context.getSignal(),
    transcription: settings.transcription,
    includeSystemAudio: settings.recording.includeSystemAudio,
    active: true,
    captureOpening: false,
  };
  context.sessionOwner = owner;
  const modelId = owner.transcription.modelId;
  context.modelUseOpening = context.acquireModelUse(modelId, owner.signal).then((grant) => {
    if (!isActive(context, owner) || !isCapturePhase(context.getState().phase)) {
      grant.release();
      return;
    }
    if (grant.status.state !== 'ready') {
      grant.release();
      throw new Error('The selected transcription model is unavailable');
    }
    context.modelUse = grant;
  });
  // The start-capture effect observes this promise. Attaching a rejection handler now keeps
  // synchronous model-loss races from becoming process-level unhandled rejections.
  void context.modelUseOpening.catch(() => undefined);
  // Warm the inference pipeline independently of capture readiness. This begins at shortcut-down
  // and overlaps the user's speech without delaying the widget, cue, helper, or microphone.
  context.warmupOpening = context.whisper.warmup?.(modelId, owner.signal) ?? Promise.resolve();
  void context.warmupOpening.catch(() => undefined);
  context.pendingSilenceSubmit = false;
  context.silence = settings.recording.autoSubmitOnSilence
    ? new SilencePolicy({
        mode: 'quick',
        preset: settings.recording.silencePreset,
      })
    : null;
  context.holdTimer = setTimeout(() => {
    if (isActive(context, owner)) context.dispatch({ type: 'hold-elapsed', now: Date.now() });
  }, ECHO_HOLD_THRESHOLD_MS);
  context.holdTimer.unref();
  context.capTimer = setTimeout(() => {
    if (isActive(context, owner)) context.dispatch({ type: 'submit', source: 'duration-cap' });
  }, SESSION_CAP_MS.extended);
  context.capTimer.unref();
}

export function observeTransition(
  context: EchoCaptureContext,
  previous: EchoSessionState,
  next: EchoSessionState,
): void {
  const owner = context.sessionOwner;
  if (previous.phase === 'arming' && next.phase === 'recordingQuick') {
    clearHoldTimer(context);
    replaceCapTimer(context, owner, SESSION_CAP_MS.quick - next.elapsedMs);
    if (context.pendingSilenceSubmit && owner !== null) {
      queueMicrotask(() => {
        if (isActive(context, owner)) context.dispatch({ type: 'submit', source: 'silence' });
      });
    }
  }
  if (next.phase === 'recordingExtended' && previous.phase !== 'recordingExtended') {
    clearHoldTimer(context);
    context.silence = null;
  }
  if (next.phase === 'transcribing' || next.phase === 'cancelled' || next.phase === 'error') {
    clearRecordingTimers(context);
  }
}

export async function startCapture(context: EchoCaptureContext): Promise<void> {
  const owner = requireActiveOwner(context);
  if (!isCapturePhase(context.getState().phase)) return;
  // Create the short-lived widget before acknowledging activation. It is removed when the
  // session ends, so each shortcut gets a new native window and compositor surface.
  const widgetPreparation = context.windows.createWidgetForActivation();
  const widgetReady =
    typeof widgetPreparation === 'boolean' ? widgetPreparation : await widgetPreparation;
  if (!isActive(context, owner)) return;
  // Show its truthful arming state before any device, model, or helper round trip so the global
  // shortcut always receives immediate visual feedback.
  if (!widgetReady || !context.windows.showWidget(context.getWidgetSize(), null)) {
    context.windows.showMain();
    throw new Error('Dictation could not start because its status widget is unavailable.');
  }
  // Model readiness, native key capture, and microphone startup are independent. Open all three
  // concurrently so none of their latencies are added together and opening speech is retained.
  const helperCaptureOpening = context.captureReconciler
    .request('recording', owner.generation)
    .catch((error: unknown) => {
      if (!context.nativeCaptureLost || !ownsSession(context, owner)) throw error;
    });
  owner.captureOpening = true;
  let capturePromise: ReturnType<EchoRecordingPort['startDictation']>;
  try {
    capturePromise = context.recording.startDictation(
      {
        onFrame: (samples, rms) => onFrame(context, owner, samples, rms),
        onUnexpectedStop: (reason) => {
          if (isActive(context, owner)) {
            context.dispatch({
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
      if (!captureStillCurrent(context, owner)) {
        await context.recording.stopDictation(capture.captureId).catch(() => undefined);
        return;
      }
      context.captureId = capture.captureId;
      if (!context.getState().audioReady) armFirstAudioTimer(context, owner);
    },
    () => {
      owner.captureOpening = false;
    },
  );

  const [, , capture] = await raceWithAbort(
    Promise.all([context.modelUseOpening, helperCaptureOpening, capturePromise]),
    owner.signal,
  );
  if (!captureStillCurrent(context, owner)) return;
  context.dispatch({
    type: 'capture-started',
    preferredUnavailable: capture.preferredUnavailable,
  });
  announceCaptureReady(context);
}

function armFirstAudioTimer(context: EchoCaptureContext, owner: CaptureSessionOwner): void {
  if (context.audioStartTimer !== null) clearTimeout(context.audioStartTimer);
  context.audioStartTimer = setTimeout(() => {
    if (!isActive(context, owner)) return;
    context.audioStartTimer = null;
    if (context.getState().audioReady || !isCapturePhase(context.getState().phase)) return;
    context.abort();
    context.dispatch({ type: 'fail', message: 'The microphone did not provide audio.' });
  }, FIRST_AUDIO_TIMEOUT_MS);
  context.audioStartTimer.unref();
}

function replaceCapTimer(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner | null,
  milliseconds: number,
): void {
  if (context.capTimer !== null) clearTimeout(context.capTimer);
  context.capTimer = setTimeout(
    () => {
      if (owner !== null && isActive(context, owner)) {
        context.dispatch({ type: 'submit', source: 'duration-cap' });
      }
    },
    Math.max(1, milliseconds),
  );
  context.capTimer.unref();
}

function clearHoldTimer(context: EchoCaptureContext): void {
  if (context.holdTimer !== null) clearTimeout(context.holdTimer);
  context.holdTimer = null;
}

export function clearRecordingTimers(context: EchoCaptureContext): void {
  clearHoldTimer(context);
  if (context.capTimer !== null) clearTimeout(context.capTimer);
  context.capTimer = null;
  if (context.audioStartTimer !== null) clearTimeout(context.audioStartTimer);
  context.audioStartTimer = null;
}
