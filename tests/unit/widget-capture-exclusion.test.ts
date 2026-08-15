import { describe, expect, it, vi } from 'vitest';
import { WidgetCaptureExclusion } from '../../app/src/main/app/widget-capture-exclusion';

describe('widget capture exclusion', () => {
  it('hides once across nested captures and restores with current placement inputs', async () => {
    let visible = true;
    let size: 'default' | 'huge' = 'default';
    const lease = { generation: 1 };
    const windows = {
      acquireWidgetVisibilityLease: vi.fn(() => (visible ? lease : null)),
      hideWidget: vi.fn(() => {
        visible = false;
      }),
      restoreWidgetVisibility: vi.fn(() => {
        visible = true;
        return true;
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
    expect(windows.restoreWidgetVisibility).not.toHaveBeenCalled();

    await exclusion.setExcluded(false);
    await exclusion.setExcluded(false);

    expect(getFrontApp).toHaveBeenCalledTimes(1);
    expect(windows.restoreWidgetVisibility).toHaveBeenCalledWith(lease, 'huge', {
      x: 1,
      y: 2,
      width: 300,
      height: 200,
    });
    expect(windows.restoreWidgetVisibility).toHaveBeenCalledTimes(1);
  });

  it('leaves an initially hidden widget hidden', async () => {
    const windows = {
      acquireWidgetVisibilityLease: vi.fn(() => null),
      hideWidget: vi.fn(),
      restoreWidgetVisibility: vi.fn(),
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
    expect(windows.restoreWidgetVisibility).not.toHaveBeenCalled();
  });

  it('restores without target bounds when front-app lookup fails', async () => {
    const lease = { generation: 1 };
    const windows = {
      acquireWidgetVisibilityLease: vi.fn(() => lease),
      hideWidget: vi.fn(),
      restoreWidgetVisibility: vi.fn(),
    };
    const exclusion = new WidgetCaptureExclusion({
      windows,
      getWidgetSize: () => 'large',
      getFrontApp: () => Promise.reject(new Error('helper unavailable')),
    });

    await exclusion.setExcluded(true);
    await exclusion.setExcluded(false);

    expect(windows.restoreWidgetVisibility).toHaveBeenCalledWith(lease, 'large', null);
  });

  it('does not restore after a terminal hide invalidates the visibility lease', async () => {
    let generation = 1;
    let visibilityDesired = true;
    let visible = true;
    let resolveFrontApp!: (value: {
      processName: string;
      windowTitle: string;
      windowBounds: { x: number; y: number; width: number; height: number };
    }) => void;
    const frontApp = new Promise<{
      processName: string;
      windowTitle: string;
      windowBounds: { x: number; y: number; width: number; height: number };
    }>((resolve) => {
      resolveFrontApp = resolve;
    });
    const windows = {
      acquireWidgetVisibilityLease: vi.fn(() => (visibilityDesired ? { generation } : null)),
      hideWidget: vi.fn((preserveInteraction = false) => {
        visible = false;
        if (preserveInteraction) return;
        visibilityDesired = false;
        generation += 1;
      }),
      restoreWidgetVisibility: vi.fn((lease: { generation: number }) => {
        if (!visibilityDesired || lease.generation !== generation) return false;
        visible = true;
        return true;
      }),
    };
    const getFrontApp = vi.fn(() => frontApp);
    const exclusion = new WidgetCaptureExclusion({
      windows,
      getWidgetSize: () => 'default',
      getFrontApp,
    });

    await exclusion.setExcluded(true);
    const restoration = exclusion.setExcluded(false);
    await Promise.resolve();
    expect(getFrontApp).toHaveBeenCalledOnce();
    windows.hideWidget(false);
    resolveFrontApp({
      processName: 'target',
      windowTitle: 'Target',
      windowBounds: { x: 1, y: 2, width: 300, height: 200 },
    });
    await restoration;

    expect(windows.restoreWidgetVisibility).toHaveReturnedWith(false);
    expect(visible).toBe(false);
  });
});
