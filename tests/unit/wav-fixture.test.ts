import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { describe, expect, it } from 'vitest';
import { parsePcm16Wav } from '../helpers/wav';

const fixtures = [
  {
    path: 'tests/fixtures/audio/speech-short-16k-mono.wav',
    samples: 8 * 16_000,
    sha256: '5e8257938418febb3687496a48ab80cac537c054cd4a52a749ade967bf8ee59a',
  },
  {
    path: 'tests/fixtures/audio/speech-boundaries-90s-16k-mono.wav',
    samples: 90 * 16_000,
    sha256: 'e1da4174bda6cc213003773fc6ef3327ec7462443d0e4fc0d951904e0b3b4c91',
  },
] as const;

describe('realistic speech WAV fixtures', () => {
  for (const fixture of fixtures) {
    it(`validates canonical PCM and hash for ${fixture.path}`, async () => {
      const bytes = await readFile(fixture.path);
      const wav = parsePcm16Wav(bytes);
      expect(wav.sampleRate).toBe(16_000);
      expect(wav.channels).toBe(1);
      expect(wav.pcm).toHaveLength(fixture.samples);
      expect(wav.pcm.some((sample) => sample !== 0)).toBe(true);
      expect(createHash('sha256').update(bytes).digest('hex')).toBe(fixture.sha256);
      expect(
        Math.max(...wav.pcm.subarray(0, Math.min(wav.pcm.length, 16_000))),
      ).toBeLessThanOrEqual(1);
      expect(
        Math.min(...wav.pcm.subarray(0, Math.min(wav.pcm.length, 16_000))),
      ).toBeGreaterThanOrEqual(-1);
    });
  }
});
