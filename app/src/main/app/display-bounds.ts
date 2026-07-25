import type { HelperFrontApp } from '../../shared/helper/protocol';

interface Point {
  readonly x: number;
  readonly y: number;
}

export function physicalBoundsToDip(
  bounds: NonNullable<HelperFrontApp['windowBounds']>,
  convert: (point: Point) => Point,
): NonNullable<HelperFrontApp['windowBounds']> {
  const topLeft = convert({ x: bounds.x, y: bounds.y });
  const bottomRight = convert({
    x: bounds.x + bounds.width,
    y: bounds.y + bounds.height,
  });
  return {
    x: topLeft.x,
    y: topLeft.y,
    width: Math.max(1, bottomRight.x - topLeft.x),
    height: Math.max(1, bottomRight.y - topLeft.y),
  };
}
