import { describe, expect, it } from 'vitest';
import { WIDGET_DIMENSIONS } from '../../app/src/shared/constants/echo-session';
import { widgetContentBounds } from '../../app/src/main/app/widget-geometry';

describe('widgetContentBounds', () => {
  it.each(Object.entries(WIDGET_DIMENSIONS))(
    'places %s at the work-area bottom center in DIPs',
    (size, dimensions) => {
      const bounds = widgetContentBounds(size as keyof typeof WIDGET_DIMENSIONS, {
        x: -1920,
        y: 40,
        width: 1920,
        height: 1040,
      });
      expect(bounds).toEqual({
        x: -1920 + Math.floor((1920 - dimensions.width) / 2),
        y: 40 + 1040 - dimensions.height - 32,
        ...dimensions,
      });
    },
  );

  it('uniformly fits Max inside a small work area without crossing any edge', () => {
    const workArea = { x: 120, y: -300, width: 320, height: 100 };
    const bounds = widgetContentBounds('max', workArea);
    expect(bounds).toEqual({ x: 120, y: -300, width: 320, height: 80 });
    expect(bounds.x).toBeGreaterThanOrEqual(workArea.x);
    expect(bounds.y).toBeGreaterThanOrEqual(workArea.y);
    expect(bounds.x + bounds.width).toBeLessThanOrEqual(workArea.x + workArea.width);
    expect(bounds.y + bounds.height).toBeLessThanOrEqual(workArea.y + workArea.height);
  });

  it('uses no bottom gap when the fitted content consumes the work-area height', () => {
    expect(widgetContentBounds('default', { x: 4, y: 8, width: 100, height: 20 })).toEqual({
      x: 16,
      y: 8,
      width: 75,
      height: 20,
    });
  });
});
