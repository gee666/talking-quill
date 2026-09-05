import { PCM_FRAME_DURATION_MS, PCM_SAMPLE_RATE } from '../../shared/constants/audio';
import { ECHO_LEVEL_EVENT_INTERVAL_MS } from '../../shared/constants/echo-session';
import { WHISPER_MAX_PUSH_SAMPLES } from '../../shared/constants/whisper';
import { TranscriptionResultSchema } from '../../shared/schemas/transcription';
import {
  assertActive,
  isActive,
  ownsSession,
  requireActiveOwner,
  type CaptureSessionOwner,
  type EchoCaptureContext,
} from './echo-capture-context';
import { announceCaptureReady } from './echo-capture-readiness';
import { abortOperationError, normalizeOperationError, raceWithAbort } from './echo-operation';
import type { WhisperStreamingSession } from './echo-session-ports';
import { concatChunks, discardChunkPrefix, sliceChunks } from './pcm-buffer';
import { publicSessionError } from './session-errors';
import { isCapturePhase, isTerminalPhase } from './session-phase';

const MAX_EXTENDED_BUFFERED_SAMPLES = WHISPER_MAX_PUSH_SAMPLES * 2;

export async function beginExtendedTranscription(context: EchoCaptureContext): Promise<void> {
  const owner = requireActiveOwner(context);
  await ensureExtendedStream(context, owner);
  if (isActive(context, owner)) queueExtendedAudio(context, owner, false);
}

export async function transcribe(context: EchoCaptureContext): Promise<string> {
  const owner = requireActiveOwner(context);
  const settings = owner.transcription;
  if (context.getState().dictationMode === 'extended') {
    await ensureExtendedStream(context, owner);
    assertActive(context, owner);
    while (context.streamedSamples < context.totalSamples || context.streamPushPending) {
      queueExtendedAudio(context, owner, true);
      const tail = context.streamTail;
      await tail;
      assertActive(context, owner);
      if (context.streamFailure !== null) break;
    }
    if (context.streamFailure !== null) {
      throw normalizeOperationError(context.streamFailure, 'Stream failed');
    }
    const stream = context.stream;
    if (stream === null) throw new Error('Transcription stream was unavailable');
    const result = await stream.finish(owner.signal);
    assertActive(context, owner);
    return TranscriptionResultSchema.parse(result).text;
  }
  if (context.totalSamples === 0) throw new Error('No audio was captured');
  const result = await context.whisper.transcribe(
    concatChunks(context.pcmChunks, context.totalSamples),
    {
      modelId: settings.modelId,
      sampleRate: PCM_SAMPLE_RATE,
      language: settings.language,
    },
    owner.signal,
  );
  assertActive(context, owner);
  return TranscriptionResultSchema.parse(result).text;
}

export function onFrame(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner,
  samples: Float32Array,
  rms: number,
): void {
  if (!isActive(context, owner)) return;
  const state = context.getState();
  const draining = state.phase === 'transcribing' && context.captureStopping;
  if (!isCapturePhase(state.phase) && !draining) return;
  const copy = Float32Array.from(samples);
  context.pcmChunks.push(copy);
  context.totalSamples += copy.length;
  if (context.audioStartTimer !== null) {
    clearTimeout(context.audioStartTimer);
    context.audioStartTimer = null;
  }
  if (!context.getState().audioReady) {
    context.dispatch({ type: 'audio-started' });
    announceCaptureReady(context);
  }
  const elapsedMs = Math.round((context.totalSamples / PCM_SAMPLE_RATE) * 1_000);
  const now = Date.now();
  if (now - context.lastLevelAt >= ECHO_LEVEL_EVENT_INTERVAL_MS) {
    context.lastLevelAt = now;
    context.dispatch({ type: 'level', rms, elapsedMs });
  }
  if (!draining && context.silence !== null) {
    const decision = context.silence.observe({
      rms,
      durationMs: (copy.length / PCM_SAMPLE_RATE) * 1_000 || PCM_FRAME_DURATION_MS,
      elapsedMs,
    });
    if (decision !== null) {
      if (context.getState().phase === 'recordingQuick') {
        context.dispatch({
          type: 'submit',
          source: decision === 'duration-cap' ? 'duration-cap' : 'silence',
        });
      } else context.pendingSilenceSubmit = true;
    }
  }
  if (context.getState().phase === 'recordingExtended') {
    if (context.totalSamples - context.discardedSamples > MAX_EXTENDED_BUFFERED_SAMPLES) {
      context.dispatch({
        type: 'fail',
        message: 'Transcription could not keep up with captured audio.',
      });
      return;
    }
    queueExtendedAudio(context, owner, false);
  }
}

async function ensureExtendedStream(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner,
): Promise<WhisperStreamingSession> {
  assertActive(context, owner);
  if (context.stream !== null) return context.stream;
  if (context.streamOpening !== null) return raceWithAbort(context.streamOpening, owner.signal);
  const settings = owner.transcription;
  const startup = context.whisper.startSession(
    {
      modelId: settings.modelId,
      sampleRate: PCM_SAMPLE_RATE,
      language: settings.language,
    },
    owner.signal,
  );
  const opening = startup.then((stream) => {
    if (!isActive(context, owner)) {
      void stream.cancel().catch(() => undefined);
      throw abortOperationError();
    }
    // Claim the stream before waking abort-bound waiters. If abort wins that race, teardown can
    // still find and cancel this stream; a later non-cooperative startup cancels itself above.
    context.stream = stream;
    return stream;
  });
  // Teardown may abandon this promise after aborting it. Keep a late worker rejection observed.
  void opening.catch(() => undefined);
  context.streamOpening = opening;
  try {
    return await raceWithAbort(opening, owner.signal);
  } finally {
    if (ownsSession(context, owner) && context.streamOpening === opening) {
      context.streamOpening = null;
    }
  }
}

function queueExtendedAudio(
  context: EchoCaptureContext,
  owner: CaptureSessionOwner,
  force: boolean,
): void {
  if (!isActive(context, owner) || context.streamPushPending) return;
  const available = context.totalSamples - context.streamedSamples;
  if (available <= 0 || (!force && available < WHISPER_MAX_PUSH_SAMPLES)) return;
  const start = context.streamedSamples;
  const count = force ? available : Math.min(available, WHISPER_MAX_PUSH_SAMPLES);
  const relativeStart = start - context.discardedSamples;
  if (relativeStart < 0) throw new Error('Extended transcription buffer accounting failed');
  const pcm = sliceChunks(context.pcmChunks, relativeStart, count);
  context.streamedSamples += count;
  context.streamPushPending = true;
  const precedingTail = context.streamTail;
  const push = precedingTail.then(async () => {
    assertActive(context, owner);
    if (context.streamFailure !== null) {
      throw normalizeOperationError(context.streamFailure, 'Transcription stream failed');
    }
    const stream = await ensureExtendedStream(context, owner);
    assertActive(context, owner);
    await stream.push(pcm, owner.signal);
    assertActive(context, owner);
    if (context.totalSamples - context.discardedSamples >= count) {
      context.pcmChunks = discardChunkPrefix(context.pcmChunks, count);
      context.discardedSamples += count;
    }
  });
  // Observe every background rejection immediately, but retain the first failure so submit can
  // never finish a stream after silently dropping audio. Only one push is retained at a time;
  // successful audio is discarded immediately to bound a 30-minute Extended session.
  context.streamTail = push
    .catch((error: unknown) => {
      if (!isActive(context, owner)) return;
      context.streamFailure ??= error;
      if (!isTerminalPhase(context.getState().phase)) {
        context.dispatch({ type: 'fail', message: publicSessionError(error) });
      }
    })
    .finally(() => {
      if (!isActive(context, owner)) return;
      context.streamPushPending = false;
      if (context.getState().phase === 'recordingExtended') {
        queueExtendedAudio(context, owner, false);
      }
    });
}
