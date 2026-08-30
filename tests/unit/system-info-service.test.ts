import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { join } from 'node:path';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const electron = vi.hoisted(() => ({
  openExternal: vi.fn<(url: string) => Promise<void>>(),
  openPath: vi.fn<(path: string) => Promise<string>>(),
  showSaveDialog: vi.fn(),
  getPath: vi.fn(() => 'downloads-root'),
}));

vi.mock('electron', () => ({
  app: { getPath: electron.getPath },
  dialog: { showSaveDialog: electron.showSaveDialog },
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
  electron.showSaveDialog.mockReset().mockResolvedValue({ canceled: true });
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

  it('exports an allowlisted ZIP with a redacted metadata file and integrity manifest', async () => {
    const root = await createTestDirectory('system-info-diagnostics');
    try {
      const logs = join(root, 'logs');
      const destination = join(root, 'report.zip');
      await mkdir(logs);
      const oldEvents = Array.from({ length: 5 }, (_, timestamp) =>
        JSON.stringify({
          timestamp,
          event: 'helper.readiness.changed',
          metadata: { component: 'helper', outcome: 'ready', reason: 'none' },
        }),
      );
      const currentEvents = Array.from({ length: 2_001 }, (_, index) =>
        JSON.stringify({
          timestamp: index + 5,
          event: 'helper.readiness.changed',
          metadata:
            index === 2_000
              ? { instanceId: 'secret-instance' }
              : { component: 'helper', outcome: 'ready', reason: 'none' },
        }),
      );
      await Promise.all([
        writeFile(join(logs, 'diagnostic.jsonl.1'), `${oldEvents.join('\n')}\n`),
        writeFile(join(logs, 'diagnostic.jsonl'), `${currentEvents.join('\n')}\n`),
      ]);
      electron.showSaveDialog.mockResolvedValue({ canceled: false, filePath: destination });
      const service = new SystemInfoService({ root, logs } as AppPaths, () => Promise.resolve());
      await expect(
        service.exportDiagnostics({} as never, {
          appVersion: '1.2.3',
          platform: 'win32',
          architecture: 'x64',
          helper: {
            status: 'unavailable',
            reason: 'owner-auth-failed',
            helperVersion: '1.2.3',
            permissions: {
              accessibility: 'unknown',
              inputMonitoring: 'unknown',
              eventPost: 'unknown',
            },
          },
          nativeLaunchFailure: null,
          settings: {
            code: 'INVALID_SETTINGS_RECOVERED',
            reason: 'parse',
            preservedAt: 'C:\\Users\\Private Person\\invalid-settings.json',
          },
        }),
      ).resolves.toBe('exported');
      const report = await readFile(destination);
      const printable = report.toString('utf8');
      expect(report.readUInt32LE(0)).toBe(0x04034b50);
      expect(printable).toContain('metadata.json');
      expect(printable).toContain('manifest.json');
      expect(printable).toContain('logs/diagnostic.jsonl');
      expect(printable).toContain('owner-auth-failed');
      expect(printable).not.toContain('secret-instance');
      expect(printable).not.toContain('Private Person');
      expect(printable).toContain('"preserved": true');
      expect(printable).toMatch(/[a-f0-9]{64}/u);
      expect(printable).not.toMatch(/voiceCommands|activationBinding|targetToken/u);
      expect(electron.showSaveDialog).toHaveBeenCalledWith(
        expect.anything(),
        expect.objectContaining({
          defaultPath: join('downloads-root', 'talking-quill-diagnostics.zip'),
        }),
      );
    } finally {
      await removeTestDirectory(root);
    }
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
