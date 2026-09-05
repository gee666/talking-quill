import { PCM_FRAME_SAMPLES } from '../../shared/constants/audio';
import type { ProcessedAudioFrame } from './audio-processing';

type CaptureWorkletMessage =
  { readonly type: 'flushed' } | (ProcessedAudioFrame & { readonly type: 'frame' });

export function readCaptureWorkletMessage(value: unknown): CaptureWorkletMessage | null {
  if (typeof value !== 'object' || value === null || !('type' in value)) return null;
  if (value.type === 'flushed') return { type: 'flushed' };
  if (value.type !== 'frame' || !('samples' in value) || !('rms' in value)) return null;
  const { samples, rms } = value;
  if (
    !(samples instanceof Float32Array) ||
    samples.length === 0 ||
    samples.length > PCM_FRAME_SAMPLES ||
    !hasNormalizedSamples(samples) ||
    typeof rms !== 'number' ||
    !Number.isFinite(rms) ||
    rms < 0 ||
    rms > 1
  ) {
    return null;
  }
  return { type: 'frame', samples, rms };
}

function hasNormalizedSamples(samples: Float32Array): boolean {
  for (const sample of samples) {
    if (!Number.isFinite(sample) || sample < -1 || sample > 1) return false;
  }
  return true;
}
