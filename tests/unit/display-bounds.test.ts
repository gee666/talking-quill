import { describe, expect, it } from 'vitest';
import { physicalBoundsToDip } from '../../app/src/main/app/display-bounds';

describe('physicalBoundsToDip', () => {
  it('converts both corners so mixed-DPI origins and dimensions stay monitor-aware', () => {
    const calls: { x: number; y: number }[] = [];
    const converted = physicalBoundsToDip(
      { x: -2400, y: 300, width: 1200, height: 900 },
      (point) => {
        calls.push(point);
        return { x: point.x / 1.5 + 100, y: point.y / 1.5 - 20 };
      },
    );
    expect(calls).toEqual([
      { x: -2400, y: 300 },
      { x: -1200, y: 1200 },
    ]);
    expect(converted).toEqual({ x: -1500, y: 180, width: 800, height: 600 });
  });

  it('guards rounding anomalies from producing empty display bounds', () => {
    expect(
      physicalBoundsToDip({ x: 1, y: 1, width: 1, height: 1 }, () => ({ x: 5, y: 5 })),
    ).toEqual({ x: 5, y: 5, width: 1, height: 1 });
  });
});
