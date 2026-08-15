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
  const createSources = () => [{ display_id: '7', thumbnail: createImage(2_000, 1_000) }];
  return {
    qualities,
    requestedSizes,
    createSources,
    desktopCapturer: {
      getSources: vi.fn((options: { thumbnailSize: { width: number; height: number } }) => {
        requestedSizes.push(options.thumbnailSize);
        return Promise.resolve(createSources());
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
    expect(electron.requestedSizes).toEqual([
      { width: SCREENSHOT_MAX_EDGE, height: SCREENSHOT_MAX_EDGE / 2 },
    ]);
    expect(Buffer.from(result.image.base64, 'base64').byteLength).toBeLessThanOrEqual(
      MAX_PROVIDER_IMAGE_BYTES,
    );
    expect(electron.qualities.every((quality) => quality === SCREENSHOT_JPEG_QUALITY)).toBe(true);
    expect(exclusions).toEqual([true, false]);
  });

  it('serializes overlapping captures so the widget stays excluded until each capture ends', async () => {
    let resolveFirst!: (sources: ReturnType<typeof electron.createSources>) => void;
    let resolveSecond!: (sources: ReturnType<typeof electron.createSources>) => void;
    electron.desktopCapturer.getSources
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveFirst = resolve;
          }),
      )
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveSecond = resolve;
          }),
      );
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    });
    const first = service.capture(
      { x: 0, y: 0, width: 10, height: 10 },
      new AbortController().signal,
    );
    const second = service.capture(
      { x: 0, y: 0, width: 10, height: 10 },
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(1));
    expect(exclusions).toEqual([true]);
    resolveFirst(electron.createSources());
    await first;
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(2));
    expect(exclusions).toEqual([true, false, true]);
    resolveSecond(electron.createSources());
    await second;
    expect(exclusions).toEqual([true, false, true, false]);
  });

  it('promptly cancels a stalled desktop capture and restores widget visibility', async () => {
    electron.desktopCapturer.getSources.mockImplementationOnce(() => new Promise(() => undefined));
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    });
    const controller = new AbortController();
    const pending = service.capture({ x: 0, y: 0, width: 10, height: 10 }, controller.signal);
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce());
    controller.abort();
    await expect(pending).rejects.toMatchObject({ code: 'CANCELLED' });
    expect(exclusions).toEqual([true, false]);
  });

  it('times out a stuck native capture, restores the widget, and recovers after the orphan settles', async () => {
    let resolveOrphan!: (sources: ReturnType<typeof electron.createSources>) => void;
    electron.desktopCapturer.getSources.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          resolveOrphan = resolve;
        }),
    );
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      nativeCaptureTimeoutMs: 20,
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    });

    const first = service.capture(
      { x: 0, y: 0, width: 10, height: 10 },
      new AbortController().signal,
    );
    const timedOut = expect(first).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce());
    await timedOut;
    expect(exclusions).toEqual([true, false]);

    await expect(
      service.capture({ x: 0, y: 0, width: 10, height: 10 }, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE' });
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce();
    expect(exclusions).toEqual([true, false]);

    resolveOrphan(electron.createSources());
    await Promise.resolve();
    await Promise.resolve();

    await expect(
      service.capture({ x: 0, y: 0, width: 10, height: 10 }, new AbortController().signal),
    ).resolves.toBeDefined();
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(2);
    expect(exclusions).toEqual([true, false, true, false]);
  });

  it('allows one bounded recovery probe without piling up captures or repeated widget hides', async () => {
    let resolveProbe!: (sources: ReturnType<typeof electron.createSources>) => void;
    electron.desktopCapturer.getSources
      .mockImplementationOnce(() => new Promise(() => undefined))
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveProbe = resolve;
          }),
      );
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      nativeCaptureTimeoutMs: 20,
      nativeRecoveryCooldownMs: 20,
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    });
    const bounds = { x: 0, y: 0, width: 10, height: 10 };

    const initial = service.capture(bounds, new AbortController().signal);
    const initialTimeout = expect(initial).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce());
    await initialTimeout;
    await expect(service.capture(bounds, new AbortController().signal)).rejects.toMatchObject({
      code: 'UNAVAILABLE',
    });
    expect(exclusions).toEqual([true, false]);

    await new Promise((resolve) => setTimeout(resolve, 25));
    const probe = service.capture(bounds, new AbortController().signal);
    const probeTimeout = expect(probe).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(2));
    await probeTimeout;
    expect(exclusions).toEqual([true, false, true, false]);

    await new Promise((resolve) => setTimeout(resolve, 25));
    await expect(service.capture(bounds, new AbortController().signal)).rejects.toMatchObject({
      code: 'UNAVAILABLE',
    });
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(2);
    expect(exclusions).toEqual([true, false, true, false]);

    resolveProbe(electron.createSources());
    await Promise.resolve();
    await Promise.resolve();
    await expect(service.capture(bounds, new AbortController().signal)).resolves.toBeDefined();
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(3);
    expect(exclusions).toEqual([true, false, true, false, true, false]);

    electron.desktopCapturer.getSources.mockImplementationOnce(() => new Promise(() => undefined));
    const laterOrphan = service.capture(bounds, new AbortController().signal);
    const laterTimeout = expect(laterOrphan).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(4));
    await laterTimeout;
    await new Promise((resolve) => setTimeout(resolve, 25));
    await expect(service.capture(bounds, new AbortController().signal)).rejects.toMatchObject({
      code: 'UNAVAILABLE',
    });
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(4);
    expect(exclusions).toEqual([true, false, true, false, true, false, true, false]);
  });

  it('retains orphan settlement recovery when a half-open probe throws synchronously', async () => {
    let resolveOrphan!: (sources: ReturnType<typeof electron.createSources>) => void;
    electron.desktopCapturer.getSources
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOrphan = resolve;
          }),
      )
      .mockImplementationOnce(() => {
        throw new Error('synchronous native failure');
      });
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      nativeCaptureTimeoutMs: 20,
      nativeRecoveryCooldownMs: 20,
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
      },
    });
    const bounds = { x: 0, y: 0, width: 10, height: 10 };

    const initial = service.capture(bounds, new AbortController().signal);
    const initialTimeout = expect(initial).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce());
    await initialTimeout;
    await new Promise((resolve) => setTimeout(resolve, 25));

    await expect(service.capture(bounds, new AbortController().signal)).rejects.toThrow(
      'synchronous native failure',
    );
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(2);
    expect(exclusions).toEqual([true, false, true, false]);

    resolveOrphan(electron.createSources());
    await Promise.resolve();
    await Promise.resolve();
    await expect(service.capture(bounds, new AbortController().signal)).resolves.toBeDefined();
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(3);
  });

  it('revalidates a half-open probe when the orphan settles during widget exclusion', async () => {
    let resolveOrphan!: (sources: ReturnType<typeof electron.createSources>) => void;
    electron.desktopCapturer.getSources
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            resolveOrphan = resolve;
          }),
      )
      .mockImplementationOnce(() => {
        throw new Error('synchronous native failure');
      });
    const exclusions: boolean[] = [];
    const service = new ScreenshotService({
      nativeCaptureTimeoutMs: 20,
      nativeRecoveryCooldownMs: 20,
      setWidgetExcluded: (excluded) => {
        exclusions.push(excluded);
        if (excluded && exclusions.filter(Boolean).length === 2) {
          resolveOrphan(electron.createSources());
        }
      },
    });
    const bounds = { x: 0, y: 0, width: 10, height: 10 };

    const initial = service.capture(bounds, new AbortController().signal);
    const initialTimeout = expect(initial).rejects.toMatchObject({ code: 'TIMEOUT' });
    await vi.waitFor(() => expect(electron.desktopCapturer.getSources).toHaveBeenCalledOnce());
    await initialTimeout;
    await new Promise((resolve) => setTimeout(resolve, 25));

    await expect(service.capture(bounds, new AbortController().signal)).rejects.toThrow(
      'synchronous native failure',
    );
    await expect(service.capture(bounds, new AbortController().signal)).resolves.toBeDefined();
    expect(electron.desktopCapturer.getSources).toHaveBeenCalledTimes(3);
    expect(exclusions).toEqual([true, false, true, false, true, false]);
  });

  it('fails closed without focused-window bounds and performs no capture', async () => {
    await expect(
      new ScreenshotService().capture(null, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'UNAVAILABLE' });
    expect(electron.desktopCapturer.getSources).not.toHaveBeenCalled();
  });
});
