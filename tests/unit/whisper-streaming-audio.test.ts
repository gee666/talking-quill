import { describe, expect, it } from 'vitest';
import {
  assertPcmLength,
  copyPcm,
  inferenceTimeoutMs,
  streamingPushPlan,
} from '../../app/src/main/transcription/whisper-streaming-audio';
import { CONTROL_REQUEST_TIMEOUT_MS } from '../../app/src/main/transcription/whisper-worker-supervisor';
import {
  WHISPER_CHUNK_SECONDS,
  WHISPER_HOP_SECONDS,
  WHISPER_SAMPLE_RATE,
} from '../../app/src/shared/constants/whisper';

const chunkSamples = WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SECONDS;
const hopSamples = WHISPER_SAMPLE_RATE * WHISPER_HOP_SECONDS;

describe('streaming audio planning', () => {
  it('keeps a control deadline until a full inference chunk is buffered', () => {
    expect(streamingPushPlan(chunkSamples - 2, 1)).toEqual({
      remainingSamples: chunkSamples - 1,
      timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
    });
    expect(streamingPushPlan(chunkSamples - 1, 1)).toEqual({
      remainingSamples: chunkSamples - hopSamples,
      timeoutMs: inferenceTimeoutMs(chunkSamples),
    });
  });

  it('budgets each overlapping inference chunk and retains its unconsumed tail', () => {
    expect(streamingPushPlan(chunkSamples - hopSamples, 2 * hopSamples + 1)).toEqual({
      remainingSamples: chunkSamples - hopSamples + 1,
      timeoutMs: inferenceTimeoutMs(2 * chunkSamples),
    });
  });

  it('rounds audio duration up before applying the inference multiplier', () => {
    expect(inferenceTimeoutMs(0)).toBe(5 * 60_000);
    expect(inferenceTimeoutMs(1)).toBe(5 * 60_000 + 3);
    expect(inferenceTimeoutMs(WHISPER_SAMPLE_RATE)).toBe(5 * 60_000 + 3_000);
  });

  it('copies only the supplied PCM view and rejects invalid lengths', () => {
    const source = Float32Array.from([1, 2, 3, 4]);
    const copy = copyPcm(source.subarray(1, 3));
    source.fill(0);
    expect([...new Float32Array(copy)]).toEqual([2, 3]);
    expect(() => assertPcmLength(new Float32Array(2), 2, 'invalid')).not.toThrow();
    for (const length of [0, 3]) {
      expect(() => assertPcmLength(new Float32Array(length), 2, 'invalid')).toThrow('invalid');
    }
  });
});
