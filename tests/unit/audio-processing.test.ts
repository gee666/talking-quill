import { describe, expect, it } from 'vitest';
import {
  StreamingPcmProcessor,
  StreamingResampler,
  calculateRms,
  downmixChannels,
} from '../../app/src/renderer/capture/audio-processing';
import { PCM_FRAME_SAMPLES, PCM_SAMPLE_RATE } from '../../app/src/shared/constants/audio';
import { WHISPER_SAMPLE_RATE } from '../../app/src/shared/constants/whisper';

describe('audio and transcription compatibility', () => {
  it('keeps capture PCM at the exact sample rate required by Whisper', () => {
    expect(PCM_SAMPLE_RATE).toBe(WHISPER_SAMPLE_RATE);
  });
});

function sine(sampleRate: number, frequency: number, seconds: number, amplitude = 1): Float32Array {
  return Float32Array.from(
    { length: Math.round(sampleRate * seconds) },
    (_value, index) => amplitude * Math.sin((2 * Math.PI * frequency * index) / sampleRate),
  );
}

function resampleInChunks(inputRate: number, input: Float32Array): Float32Array {
  const resampler = new StreamingResampler(inputRate, 16_000);
  const chunks: Float32Array[] = [];
  let offset = 0;
  const sizes = [1, 127, 128, 511, 37];
  let sizeIndex = 0;
  while (offset < input.length) {
    const size = Math.min(sizes[sizeIndex % sizes.length] ?? 128, input.length - offset);
    chunks.push(resampler.process(input.slice(offset, offset + size)));
    offset += size;
    sizeIndex += 1;
  }
  chunks.push(resampler.flush());
  const length = chunks.reduce((total, chunk) => total + chunk.length, 0);
  const output = new Float32Array(length);
  let outputOffset = 0;
  for (const chunk of chunks) {
    output.set(chunk, outputOffset);
    outputOffset += chunk.length;
  }
  return output;
}

describe('capture audio processing', () => {
  it.each([44_100, 48_000])(
    'resamples chunked %s Hz input continuously to an exact 16 kHz sample count',
    (inputRate) => {
      const input = sine(inputRate, 1_000, 2.25, 0.5);
      const output = resampleInChunks(inputRate, input);
      expect(output).toHaveLength(36_000);
      expect(output.every(Number.isFinite)).toBe(true);
      expect(calculateRms(output.slice(1_000, -1_000))).toBeCloseTo(Math.SQRT1_2 * 0.5, 2);
      let phaseError = 0;
      for (let index = 1_000; index < output.length - 1_000; index += 1) {
        const expected = 0.5 * Math.sin((2 * Math.PI * 1_000 * index) / 16_000);
        phaseError += Math.abs((output[index] ?? 0) - expected);
      }
      expect(phaseError / (output.length - 2_000)).toBeLessThan(0.03);
    },
  );

  it('attenuates frequencies above the 16 kHz Nyquist limit', () => {
    const output = resampleInChunks(48_000, sine(48_000, 12_000, 1, 0.8));
    expect(calculateRms(output.slice(1_000, -1_000))).toBeLessThan(0.08);
  });

  it('averages every source channel, sanitizes samples, and computes RMS', () => {
    const mixed = downmixChannels([
      Float32Array.from([1, 0.5, Number.NaN, -1]),
      Float32Array.from([-1, 0.5, 1, 1]),
    ]);
    expect([...mixed]).toEqual([0, 0.5, 0.5, 0]);
    expect(calculateRms(Float32Array.from([1, -1, 1, -1]))).toBe(1);
    expect(calculateRms(new Float32Array())).toBe(0);
  });

  it('emits fixed 320-sample frames plus one final flushed partial frame with RMS', () => {
    const frames: { readonly samples: Float32Array; readonly rms: number }[] = [];
    const processor = new StreamingPcmProcessor(48_000, (frame) => frames.push(frame));
    const input = Float32Array.from({ length: 48_000 + 150 }, () => 0.25);
    for (let offset = 0; offset < input.length; offset += 128) {
      processor.process([input.slice(offset, offset + 128)]);
    }
    processor.flush();
    expect(frames.slice(0, -1).every((frame) => frame.samples.length === PCM_FRAME_SAMPLES)).toBe(
      true,
    );
    expect(frames.at(-1)?.samples.length).toBeGreaterThan(0);
    expect(frames.reduce((total, frame) => total + frame.samples.length, 0)).toBe(
      Math.floor((input.length * 16_000) / 48_000),
    );
    expect(frames[2]?.rms).toBeCloseTo(0.25, 2);
  });
});
