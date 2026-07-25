import type { Rectangle } from 'electron';
import { WIDGET_DIMENSIONS } from '../../shared/constants/echo-session';
import type { Settings } from '../../shared/schemas/settings';

const WORK_AREA_BOTTOM_GAP = 32;

/** Computes integer DIP content bounds fully contained by the selected display work area. */
export function widgetContentBounds(
  size: Settings['app']['widgetSize'],
  workArea: Rectangle,
): Rectangle {
  const requested = WIDGET_DIMENSIONS[size];
  const fit = Math.min(1, workArea.width / requested.width, workArea.height / requested.height);
  const width = Math.max(1, Math.floor(requested.width * fit));
  const height = Math.max(1, Math.floor(requested.height * fit));
  const gap = Math.max(0, Math.min(WORK_AREA_BOTTOM_GAP, workArea.height - height));
  return {
    x: workArea.x + Math.floor((workArea.width - width) / 2),
    y: workArea.y + workArea.height - height - gap,
    width,
    height,
  };
}
