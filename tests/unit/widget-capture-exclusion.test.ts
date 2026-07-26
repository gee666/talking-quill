import { describe, expect, it, vi } from 'vitest';
import { WidgetCaptureExclusion } from '../../app/src/main/app/widget-capture-exclusion';

describe('widget capture exclusion', () => {
  it('hides once across nested captures and restores with current placement inputs', async () => {
    let visible = true;
    let size: 'default' | 'huge' = 'default';
    const windows = {
      isWidgetVisible: vi.fn(() => visible),
      hideWidget: vi.fn(() => {
        visible = false;
      }),
      showWidget: vi.fn(() => {
        visible = true;
      }),
    };
    const getFrontApp = vi.fn(() =>
      Promise.resolve({
        processName: 'target',
        windowTitle: 'Target',
        windowBounds: { x: 1, y: 2, width: 300, height: 200 },
      }),
    );
    const exclusion = new WidgetCaptureExclusion({
      windows,
      getWidgetSize: () => size,
      getFrontApp,
    });

    await exclusion.setExcluded(true);
    await exclusion.setExcluded(true);
    size = 'huge';
    await exclusion.setExcluded(false);

    expect(windows.hideWidget).toHaveBeenCalledTimes(1);
    expect(getFrontApp).not.toHaveBeenCalled();
    expect(windows.showWidget).not.toHaveBeenCalled();

    await exclusion.setExcluded(false);
    await exclusion.setExcluded(false);

    expect(getFrontApp).toHaveBeenCalledTimes(1);
    expect(windows.showWidget).toHaveBeenCalledWith('huge', {
      x: 1,
      y: 2,
      width: 300,
      height: 200,
    });
    expect(windows.showWidget).toHaveBeenCalledTimes(1);
  });

  it('leaves an initially hidden widget hidden', async () => {
    const windows = {
      isWidgetVisible: vi.fn(() => false),
      hideWidget: vi.fn(),
      showWidget: vi.fn(),
    };
    const getFrontApp = vi.fn(() => Promise.reject(new Error('helper unavailable')));
    const exclusion = new WidgetCaptureExclusion({
      windows,
      getWidgetSize: () => 'default',
      getFrontApp,
    });

    await exclusion.setExcluded(true);
    await exclusion.setExcluded(false);

    expect(windows.hideWidget).toHaveBeenCalledTimes(1);
    expect(getFrontApp).not.toHaveBeenCalled();
    expect(windows.showWidget).not.toHaveBeenCalled();
  });

  it('restores without target bounds when front-app lookup fails', async () => {
    const windows = {
      isWidgetVisible: vi.fn(() => true),
      hideWidget: vi.fn(),
      showWidget: vi.fn(),
    };
    const exclusion = new WidgetCaptureExclusion({
      windows,
      getWidgetSize: () => 'large',
      getFrontApp: () => Promise.reject(new Error('helper unavailable')),
    });

    await exclusion.setExcluded(true);
    await exclusion.setExcluded(false);

    expect(windows.showWidget).toHaveBeenCalledWith('large', null);
  });
});
