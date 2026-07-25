import { beforeEach, describe, expect, it, vi } from 'vitest';

const electron = vi.hoisted(() => {
  const qualities: number[] = [];
  const requestedSizes: { width: number; height: number }[] = [];
  const createImage = (width: number, height: number) => ({
    getSize: () => ({ width, height }),
    resize: (size: { width: number; height: number }) => createImage(size.width, size.height),
    toJPEG: (quality: number) => {
      qualities.push(quality);
      return Buffer.alloc(Math.max(64, Math.ceil((width * height) / 4)));
    },
  });
  return {
    qualities,
    requestedSizes,
    desktopCapturer: {
      getSources: vi.fn((options: { thumbnailSize: { width: number; height: number } }) => {
        requestedSizes.push(options.thumbnailSize);
        return Promise.resolve([{ display_id: '7', thumbnail: createImage(2_000, 1_000) }]);
      }),
    },
    screen: {
      getDisplayMatching: vi.fn(() => ({
        id: 7,
        bounds: { x: 0, y: 0, width: 2_000, height: 1_000 },
        scaleFactor: 1,
      })),
      screenToDipPoint: (point: { x: number; y: number }) => point,
    },
    systemPreferences: { getMediaAccessStatus: () => 'granted' },
  };
});

vi.mock('electron', () => ({
  desktopCapturer: electron.desktopCapturer,
  screen: electron.screen,
  systemPreferences: electron.systemPreferences,
}));

import { MAX_PROVIDER_IMAGE_BYTES } from '../../app/src/shared/schemas/providers';
import {
  SCREENSHOT_JPEG_QUALITY,
  SCREENSHOT_MAX_EDGE,
  ScreenshotService,
} from '../../app/src/main/screenshot/screenshot-service';

beforeEach(() => {
  electron.qualities.length = 0;
  electron.requestedSizes.length = 0;
  electron.desktopCapturer.getSources.mockClear();
  electron.screen.getDisplayMatching.mockClear();
});

describe('ScreenshotService', () => {
  it('captures only the focused display, excludes the widget, and enforces image bounds', async () => {
    const exclusions: boolean[] = [];
    const result = await new ScreenshotService({
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    }).capture({ x: 25, y: 40, width: 800, height: 600 }, new AbortController().signal);

    expect(electron.screen.getDisplayMatching).toHaveBeenCalledTimes(1);
    expect(electron.requestedSizes).toEqual([{ width: 1_568, height: 784 }]);
    expect(result.width).toBeLessThanOrEqual(SCREENSHOT_MAX_EDGE);
    expect(result.height).toBeLessThanOrEqual(SCREENSHOT_MAX_EDGE);
    expect(Buffer.from(result.image.base64, 'base64').byteLength).toBeLessThanOrEqual(
      MAX_PROVIDER_IMAGE_BYTES,
    );
    expect(electron.qualities.every((quality) => quality === SCREENSHOT_JPEG_QUALITY)).toBe(true);
    expect(exclusions).toEqual([true, false]);
  });

  it('fails closed without focused-window bounds and performs no capture', async () => {
    await expect(
      new ScreenshotService().capture(null, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE' });
    expect(electron.desktopCapturer.getSources).not.toHaveBeenCalled();
  });
});
