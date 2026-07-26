import { beforeEach, describe, expect, it, vi } from 'vitest';

const electron = vi.hoisted(() => ({
  openExternal: vi.fn<(url: string) => Promise<void>>(),
  openPath: vi.fn<(path: string) => Promise<string>>(),
}));

vi.mock('electron', () => ({
  shell: {
    openExternal: electron.openExternal,
    openPath: electron.openPath,
  },
}));

import type { AppPaths } from '../../app/src/main/persistence/paths';
import { SystemInfoService } from '../../app/src/main/info/system-info-service';

const paths = { root: 'data-root', logs: 'logs-root' } as AppPaths;

beforeEach(() => {
  electron.openExternal.mockReset().mockResolvedValue(undefined);
  electron.openPath.mockReset().mockResolvedValue('');
});

describe('SystemInfoService', () => {
  it('opens only the selected fixed data location', async () => {
    const service = new SystemInfoService(paths, () => Promise.resolve());
    await service.openLocation('data');
    await service.openLocation('logs');
    expect(electron.openPath).toHaveBeenNthCalledWith(1, 'data-root');
    expect(electron.openPath).toHaveBeenNthCalledWith(2, 'logs-root');
  });

  it('maps resolved and rejected shell failures to UNAVAILABLE', async () => {
    const service = new SystemInfoService(paths, () => Promise.resolve());
    electron.openPath.mockResolvedValueOnce('OS error');
    await expect(service.openLocation('data')).rejects.toMatchObject({
      publicError: { code: 'UNAVAILABLE' },
    });
    electron.openPath.mockRejectedValueOnce(new Error('shell failed'));
    await expect(service.openLocation('logs')).rejects.toMatchObject({
      publicError: { code: 'UNAVAILABLE' },
    });
    electron.openExternal.mockRejectedValueOnce(new Error('shell failed'));
    await expect(
      service.openRelease('https://github.com/gee666/talking-quill/releases/tag/v1.2.3'),
    ).rejects.toMatchObject({ publicError: { code: 'UNAVAILABLE' } });
  });

  it('maps microphone settings failures without weakening release URL validation', async () => {
    const openMicrophoneSettings = vi.fn().mockRejectedValue(new Error('settings failed'));
    const service = new SystemInfoService(paths, openMicrophoneSettings);
    await expect(service.openPermission('microphone')).rejects.toMatchObject({
      publicError: { code: 'UNAVAILABLE' },
    });
    await expect(service.openRelease('https://example.invalid/release')).rejects.toMatchObject({
      publicError: { code: 'FORBIDDEN' },
    });
    expect(electron.openExternal).not.toHaveBeenCalled();
  });
});
