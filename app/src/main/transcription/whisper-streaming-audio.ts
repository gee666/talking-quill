import {
  WHISPER_SAMPLE_RATE,
  WHISPER_CHUNK_SECONDS,
  WHISPER_HOP_SECONDS,
} from '../../shared/constants/whisper';
import { WhisperClientError } from './errors';
import { CONTROL_REQUEST_TIMEOUT_MS } from './whisper-worker-requests-support';

const INFERENCE_STARTUP_TIMEOUT_MS = 5 * 60_000;
const INFERENCE_REALTIME_MULTIPLIER = 3;

export function inferenceTimeoutMs(sampleCount: number): number {
  const audioDurationMs = Math.ceil((sampleCount * 1_000) / WHISPER_SAMPLE_RATE);
  return INFERENCE_STARTUP_TIMEOUT_MS + audioDurationMs * INFERENCE_REALTIME_MULTIPLIER;
}

export function combineAbortSignals(
  first: AbortSignal | undefined,
  second: AbortSignal,
): AbortSignal {
  return first === undefined ? second : AbortSignal.any([first, second]);
}

export function streamingPushPlan(
  bufferedSamples: number,
  pushedSamples: number,
): {
  readonly remainingSamples: number;
  readonly timeoutMs: number;
} {
  const chunkSamples = WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SECONDS;
  const hopSamples = WHISPER_SAMPLE_RATE * WHISPER_HOP_SECONDS;
  let remainingSamples = bufferedSamples + pushedSamples;
  let inferenceSamples = 0;
  while (remainingSamples >= chunkSamples) {
    inferenceSamples += chunkSamples;
    remainingSamples -= hopSamples;
  }
  return {
    remainingSamples,
    timeoutMs:
      inferenceSamples === 0 ? CONTROL_REQUEST_TIMEOUT_MS : inferenceTimeoutMs(inferenceSamples),
  };
}

export function assertPcmLength(pcm: Float32Array, maximum: number, message: string): void {
  if (pcm.length === 0 || pcm.length > maximum) {
    throw new WhisperClientError('INVALID_AUDIO', message);
  }
}

export function copyPcm(pcm: Float32Array): ArrayBuffer {
  const copy = new Float32Array(pcm.length);
  copy.set(pcm);
  return copy.buffer;
}

export function once(operation: () => void): () => void {
  let called = false;
  return () => {
    if (called) return;
    called = true;
    operation();
  };
}
