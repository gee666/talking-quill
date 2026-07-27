import { describe, expect, it } from 'vitest';
import { SilencePolicy } from '../../app/src/main/audio/silence-policy';
import { SESSION_CAP_MS, SILENCE_PRESET_MS } from '../../app/src/shared/constants/audio';

function feed(
  policy: SilencePolicy,
  rms: number,
  durationMs: number,
  startElapsedMs: number,
): ReturnType<SilencePolicy['observe']> {
  let decision: ReturnType<SilencePolicy['observe']> = null;
  for (let elapsed = 20; elapsed <= durationMs; elapsed += 20) {
    decision = policy.observe({ rms, durationMs: 20, elapsedMs: startElapsedMs + elapsed });
  }
  return decision;
}

describe('SilencePolicy', () => {
  it('publishes the exact product timing constants', () => {
    expect(SILENCE_PRESET_MS).toEqual({ aggressive: 1_000, average: 1_800, relaxed: 3_000 });
    expect(SESSION_CAP_MS).toEqual({ quick: 120_000, extended: 1_800_000 });
  });

  it.each([
    ['aggressive', 1_000],
    ['average', 1_800],
    ['relaxed', 3_000],
  ] as const)(
    'applies the %s trailing-silence boundary only after speech arming',
    (preset, silenceMs) => {
      const policy = new SilencePolicy({ mode: 'quick', preset });
      expect(feed(policy, 0.001, 2_000, 0)).toBeNull();
      expect(policy.armed).toBe(false);
      expect(feed(policy, 0.1, 280, 2_000)).toBeNull();
      expect(policy.armed).toBe(false);
      expect(feed(policy, 0.1, 20, 2_280)).toBeNull();
      expect(policy.armed).toBe(true);
      expect(feed(policy, 0.001, silenceMs - 20, 2_300)).toBeNull();
      expect(feed(policy, 0.001, 20, 2_300 + silenceMs - 20)).toBe('trailing-silence');
    },
  );

  it('requires consecutive speech through both silence and the hysteresis band', () => {
    const policy = new SilencePolicy({ mode: 'quick', preset: 'aggressive' });
    feed(policy, 0.1, 200, 0);
    feed(policy, 0.01, 20, 200);
    feed(policy, 0.1, 100, 220);
    expect(policy.armed).toBe(false);
    feed(policy, 0.1, 300, 320);
    expect(policy.armed).toBe(true);
    feed(policy, 0.001, 800, 620);
    feed(policy, 0.1, 20, 1_420);
    expect(feed(policy, 0.001, 980, 1_440)).toBeNull();
    expect(feed(policy, 0.001, 20, 2_420)).toBe('trailing-silence');
  });

  it('eventually counts sustained hysteresis-band room noise as trailing silence', () => {
    const policy = new SilencePolicy({ mode: 'quick', preset: 'aggressive' });
    feed(policy, 0.1, 300, 0);
    expect(policy.armed).toBe(true);
    expect(feed(policy, 0.01, 980, 300)).toBeNull();
    expect(feed(policy, 0.01, 20, 1_280)).toBe('trailing-silence');
  });

  it('does not count a brief hysteresis-band dip when clear speech resumes', () => {
    const policy = new SilencePolicy({ mode: 'quick', preset: 'aggressive' });
    feed(policy, 0.1, 300, 0);
    feed(policy, 0.01, 280, 300);
    feed(policy, 0.1, 20, 580);
    expect(feed(policy, 0.001, 980, 600)).toBeNull();
    expect(feed(policy, 0.001, 20, 1_580)).toBe('trailing-silence');
  });

  it('adapts a bounded noise floor without learning speech as noise', () => {
    const policy = new SilencePolicy({ mode: 'quick', preset: 'average' });
    const initialFloor = policy.noiseFloor;
    feed(policy, 0.006, 5_000, 0);
    expect(policy.noiseFloor).toBeGreaterThan(initialFloor);
    expect(policy.noiseFloor).toBeLessThan(0.0061);
    feed(policy, 0.12, 300, 5_000);
    expect(policy.armed).toBe(true);
    expect(policy.noiseFloor).toBeLessThan(0.01);
    const armedFloor = policy.noiseFloor;
    feed(policy, 0.006, 1_000, 5_300);
    expect(policy.noiseFloor).toBe(armedFloor);
  });

  it.each([
    ['quick', 120_000],
    ['extended', 1_800_000],
  ] as const)('enforces the %s wall-clock hard cap exactly once', (mode, capMs) => {
    const policy = new SilencePolicy({ mode, preset: 'average' });
    expect(policy.tick(capMs - 1)).toBeNull();
    expect(policy.tick(capMs)).toBe('duration-cap');
    expect(policy.observe({ rms: 0.1, durationMs: 20, elapsedMs: capMs + 20 })).toBe(
      'duration-cap',
    );
  });

  it('never auto-submits Extended mode for silence', () => {
    const policy = new SilencePolicy({ mode: 'extended', preset: 'aggressive' });
    feed(policy, 0.1, 300, 0);
    expect(feed(policy, 0, 30_000, 300)).toBeNull();
  });

  it('rejects malformed observations', () => {
    const policy = new SilencePolicy({ mode: 'quick', preset: 'average' });
    expect(() => policy.observe({ rms: Number.NaN, durationMs: 20, elapsedMs: 20 })).toThrow();
    expect(() => policy.observe({ rms: 0, durationMs: 0, elapsedMs: 20 })).toThrow();
    expect(() => policy.tick(-1)).toThrow();
  });
});
