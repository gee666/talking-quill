import { describe, expect, it, vi } from 'vitest';

const electron = vi.hoisted(() => {
  interface MenuItem {
    readonly label?: string;
    readonly enabled?: boolean;
    readonly click?: () => void;
  }
  const templates: MenuItem[][] = [];
  const destroy = vi.fn();
  const setTemplateImage = vi.fn();
  const addRepresentation = vi.fn();
  const isEmpty = vi.fn(() => false);
  class Tray {
    setToolTip = vi.fn();
    on = vi.fn();
    setContextMenu = vi.fn();
    destroy = destroy;
  }
  return {
    templates,
    destroy,
    setTemplateImage,
    addRepresentation,
    isEmpty,
    Menu: {
      buildFromTemplate: vi.fn((template: MenuItem[]) => {
        templates.push(template);
        return template;
      }),
    },
    Tray,
    nativeImage: {
      createFromDataURL: vi.fn((dataURL: string) => ({
        dataURL,
        setTemplateImage,
        addRepresentation,
        isEmpty,
        resize: vi.fn(() => ({})),
      })),
    },
  };
});

vi.mock('electron', () => ({
  Menu: electron.Menu,
  Tray: electron.Tray,
  nativeImage: electron.nativeImage,
}));

import { TrayController } from '../../app/src/main/app/tray-controller';
import { TRAY_ICON_DATA_URLS, createTrayImage } from '../../app/src/main/app/tray-icon';
import type { AppStateService } from '../../app/src/main/app/app-state-service';

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function dataUrlCalls(): string[] {
  const calls = electron.nativeImage.createFromDataURL.mock.calls as unknown as [string][];
  return calls.map(([url]) => url);
}

describe('tray icon image', () => {
  it('builds the tray image from PNG data URLs, never SVG, with a HiDPI representation', () => {
    electron.nativeImage.createFromDataURL.mockClear();
    electron.addRepresentation.mockClear();
    electron.setTemplateImage.mockClear();

    createTrayImage();

    // Template images are a macOS concept; on Windows they render as a black blob.
    expect(electron.setTemplateImage).toHaveBeenCalledWith(process.platform === 'darwin');

    const url = dataUrlCalls().at(0) ?? '';
    expect(url.startsWith('data:image/png;base64,')).toBe(true);
    expect(url).not.toContain('svg');
    expect(electron.addRepresentation).toHaveBeenCalledWith({
      scaleFactor: 2,
      dataURL: expect.stringContaining('data:image/png;base64,') as string,
    });

    for (const dataUrl of Object.values(TRAY_ICON_DATA_URLS)) {
      expect(dataUrl.startsWith('data:image/png;base64,')).toBe(true);
      const bytes = Buffer.from(dataUrl.slice('data:image/png;base64,'.length), 'base64');
      expect([...bytes.subarray(0, 8)]).toEqual([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
      expect(bytes.toString('latin1', 12, 16)).toBe('IHDR');
      expect(bytes.readUInt32BE(16)).toBe(bytes.readUInt32BE(20));
      expect([16, 32]).toContain(bytes.readUInt32BE(16));
    }
  });

  it('falls back to a guaranteed-valid PNG when decoding yields an empty image', () => {
    electron.nativeImage.createFromDataURL.mockClear();
    electron.addRepresentation.mockClear();
    electron.setTemplateImage.mockClear();
    electron.isEmpty.mockReturnValueOnce(true);

    createTrayImage();

    const darwin = process.platform === 'darwin';
    const urls = dataUrlCalls();
    expect(urls).toHaveLength(2);
    expect(urls.at(1)).toBe(
      darwin ? TRAY_ICON_DATA_URLS.fallbackTemplate16 : TRAY_ICON_DATA_URLS.fallback16,
    );
    // The fallback path still gets a HiDPI representation.
    expect(electron.addRepresentation).toHaveBeenCalledWith({
      scaleFactor: 2,
      dataURL: darwin ? TRAY_ICON_DATA_URLS.fallbackTemplate32 : TRAY_ICON_DATA_URLS.fallback32,
    });
    expect(electron.setTemplateImage).toHaveBeenCalledWith(darwin);
  });
});

describe('TrayController lifecycle', () => {
  it('stops new mutations, drains the active mutation, and never refreshes after destruction', async () => {
    const mutation = deferred();
    const setEnabled = vi.fn(() => mutation.promise);
    const controller = new TrayController(
      { getState: () => ({ enabled: true }) } as unknown as AppStateService,
      { showMain: vi.fn(), quit: vi.fn(), setEnabled },
    );
    const firstMenu = electron.templates.at(-1);
    const firstToggle = firstMenu?.find(({ label }) => label === 'Disable');

    firstToggle?.click?.();
    expect(setEnabled).toHaveBeenCalledOnce();
    controller.stopAccepting();
    const stoppedMenu = electron.templates.at(-1);
    const stoppedToggle = stoppedMenu?.find(({ label }) => label === 'Disable');
    expect(stoppedToggle?.enabled).toBe(false);
    stoppedToggle?.click?.();
    expect(setEnabled).toHaveBeenCalledOnce();

    let drained = false;
    const drain = controller.drain().then(() => {
      drained = true;
    });
    await Promise.resolve();
    expect(drained).toBe(false);
    const refreshCount = electron.templates.length;
    controller.destroy();
    controller.destroy();
    mutation.resolve();
    await drain;

    expect(drained).toBe(true);
    expect(electron.templates).toHaveLength(refreshCount);
    expect(electron.destroy).toHaveBeenCalledOnce();
  });
});
