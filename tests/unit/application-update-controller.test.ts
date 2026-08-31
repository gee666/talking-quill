import { describe, expect, it, vi } from 'vitest';
import {
  ApplicationUpdateController,
  type ApplicationUpdateBackend,
} from '../../app/src/main/info/application-update-controller';
import {
  MacosMaintenancePostponedError,
  type DownloadedApplicationUpdate,
} from '../../app/src/main/info/macos-owner-update-coordinator';

function backend(version = '1.1.0') {
  let progress: ((percent: number) => void) | null = null;
  let error: (() => void) | null = null;
  const checkForUpdates = vi.fn<
    () => Promise<{ readonly version: string; readonly releaseUrl: string | null } | null>
  >(() =>
    Promise.resolve({
      version,
      releaseUrl: `https://github.com/gee666/talking-quill/releases/tag/v${version}`,
    }),
  );
  const downloadUpdate = vi.fn<() => Promise<DownloadedApplicationUpdate | undefined>>(() =>
    Promise.resolve({ files: ['/tmp/Talking-Quill-update'] }),
  );
  const quitAndInstall = vi.fn();
  const requestElevation = vi.fn<() => Promise<'accepted' | 'cancelled'>>(() =>
    Promise.resolve('accepted'),
  );
  const value: ApplicationUpdateBackend = {
    checkForUpdates,
    downloadUpdate,
    requestElevation,
    quitAndInstall,
    onProgress(listener) {
      progress = listener;
      return () => {
        progress = null;
      };
    },
    onError(listener) {
      error = listener;
      return () => {
        error = null;
      };
    },
    dispose: vi.fn(),
  };
  return {
    value,
    checkForUpdates,
    downloadUpdate,
    requestElevation,
    quitAndInstall,
    progress: (percent: number) => progress?.(percent),
    error: () => error?.(),
  };
}

const available = {
  status: 'available',
  currentVersion: '1.0.0',
  latestVersion: '1.1.0',
  releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
} as const;

describe('application update consent and installation controller', () => {
  it('automatically checks/downloads, then requests a drained maintenance install on consent', async () => {
    const updater = backend();
    let finishDownload!: (value: DownloadedApplicationUpdate) => void;
    updater.downloadUpdate.mockReturnValueOnce(
      new Promise((resolve) => {
        finishDownload = resolve;
      }),
    );
    const publish = vi.fn();
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish,
      requestInstall,
    });

    await expect(controller.acceptCheckResult(available)).resolves.toMatchObject({
      phase: 'available',
      availableVersion: '1.1.0',
    });
    expect(updater.checkForUpdates).toHaveBeenCalledOnce();
    updater.progress(52.5);
    expect(controller.getState()).toMatchObject({ phase: 'downloading', percent: 52.5 });
    finishDownload({ files: ['/tmp/Talking-Quill-update'] });
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    expect(updater.downloadUpdate).toHaveBeenCalledOnce();
    expect(controller.apply().phase).toBe('installing');
    await vi.waitFor(() => expect(requestInstall).toHaveBeenCalledOnce());
    expect(controller.getState()).toMatchObject({ phase: 'installing', percent: 100 });
    controller.quitAndInstall();
    expect(updater.quitAndInstall).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenCalled();
  });

  it('publishes the compatible edge and URL before its download starts', async () => {
    const updater = backend('1.1.0');
    const publish = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish,
      requestInstall: vi.fn(),
    });
    await controller.acceptCheckResult({ ...available, latestVersion: '1.2.0' });
    expect(publish).toHaveBeenNthCalledWith(
      1,
      expect.objectContaining({
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
        latestVersion: '1.2.0',
      }),
    );
    expect(publish.mock.invocationCallOrder[0]).toBeLessThan(
      updater.downloadUpdate.mock.invocationCallOrder[0] ?? Number.POSITIVE_INFINITY,
    );
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    expect(controller.getState()).toMatchObject({
      availableVersion: '1.1.0',
      releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
      latestVersion: '1.2.0',
      latestReleaseUrl: available.releaseUrl,
    });
  });

  it('preserves macOS owner updates when the backend has no Windows publication URL', async () => {
    const updater = backend('1.1.0');
    updater.checkForUpdates.mockResolvedValueOnce({ version: '1.1.0', releaseUrl: null });
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall: vi.fn(),
    });
    await expect(controller.acceptCheckResult(available)).resolves.toMatchObject({
      availableVersion: '1.1.0',
      releaseUrl: available.releaseUrl,
    });
    expect(updater.downloadUpdate).toHaveBeenCalledOnce();
  });

  it('rejects an intermediate edge without its own signed publication URL', async () => {
    const updater = backend('1.1.0');
    updater.checkForUpdates.mockResolvedValueOnce({ version: '1.1.0', releaseUrl: null });
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall: vi.fn(),
    });
    await controller.acceptCheckResult({ ...available, latestVersion: '1.2.0' });
    expect(controller.getState()).toMatchObject({ phase: 'error', releaseUrl: null });
    expect(updater.downloadUpdate).not.toHaveBeenCalled();
  });

  it('requires native candidate preparation before requesting installation', async () => {
    const updater = backend();
    updater.downloadUpdate.mockResolvedValueOnce({ files: ['/tmp/Talking-Quill.zip'] });
    const prepareInstall = vi.fn(() => Promise.resolve());
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall,
      prepareInstall,
    });
    await controller.acceptCheckResult(available);
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    controller.apply();
    await vi.waitFor(() => expect(requestInstall).toHaveBeenCalledOnce());
    expect(prepareInstall).toHaveBeenCalledWith({ files: ['/tmp/Talking-Quill.zip'] });
    expect(prepareInstall.mock.invocationCallOrder[0]).toBeLessThan(
      requestInstall.mock.invocationCallOrder[0] ?? Number.POSITIVE_INFINITY,
    );
  });

  it('keeps the update available and the application running when UAC is cancelled', async () => {
    const updater = backend();
    updater.requestElevation.mockResolvedValueOnce('cancelled');
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall,
    });
    await controller.acceptCheckResult(available);
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    controller.apply();
    await vi.waitFor(() => expect(controller.getState().message).toContain('cancelled'));
    expect(controller.getState().phase).toBe('available');
    expect(requestInstall).not.toHaveBeenCalled();
    expect(updater.quitAndInstall).not.toHaveBeenCalled();
  });

  it('does not request installation after a backend error cancels an in-flight elevation', async () => {
    const updater = backend();
    let resolveElevation!: (value: 'accepted') => void;
    updater.requestElevation.mockReturnValueOnce(
      new Promise<'accepted'>((resolve) => {
        resolveElevation = resolve;
      }),
    );
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall,
    });
    await controller.acceptCheckResult(available);
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    controller.apply();
    await vi.waitFor(() => expect(updater.requestElevation).toHaveBeenCalledOnce());
    updater.error();
    resolveElevation('accepted');
    await vi.waitFor(() => expect(controller.getState().phase).toBe('error'));
    expect(requestInstall).not.toHaveBeenCalled();
  });

  it('reports held-key maintenance postponement without requesting installation', async () => {
    const updater = backend();
    updater.downloadUpdate.mockResolvedValueOnce({ files: ['/tmp/Talking-Quill.zip'] });
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall,
      prepareInstall: () => Promise.reject(new MacosMaintenancePostponedError()),
    });
    await controller.acceptCheckResult(available);
    await vi.waitFor(() => expect(controller.getState().phase).toBe('available'));
    controller.apply();
    await vi.waitFor(() => expect(controller.getState().phase).toBe('error'));
    expect(controller.getState().message).toContain('Release all Talking Quill shortcut keys');
    expect(requestInstall).not.toHaveBeenCalled();
  });

  it('serializes compatible-edge selection before download ownership', async () => {
    const updater = backend('1.1.0');
    let finishCheck!: (value: { version: string; releaseUrl: string }) => void;
    updater.checkForUpdates.mockReturnValueOnce(
      new Promise((resolve) => {
        finishCheck = resolve;
      }),
    );
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall: vi.fn(),
    });
    const first = controller.acceptCheckResult(available);
    await expect(controller.acceptCheckResult(available)).resolves.toMatchObject({ phase: 'idle' });
    expect(updater.checkForUpdates).toHaveBeenCalledOnce();
    finishCheck({ version: '1.1.0', releaseUrl: available.releaseUrl });
    await first;
    expect(updater.downloadUpdate).toHaveBeenCalledOnce();
  });

  it('fails closed when updater metadata does not match the release check', async () => {
    const updater = backend('1.2.0');
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish: vi.fn(),
      requestInstall,
    });
    await controller.acceptCheckResult(available);
    await vi.waitFor(() => expect(controller.getState().phase).toBe('error'));
    expect(updater.downloadUpdate).not.toHaveBeenCalled();
    expect(requestInstall).not.toHaveBeenCalled();
  });

  it('never claims automatic installation without a packaged updater backend', async () => {
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: null,
      publish: vi.fn(),
      requestInstall: vi.fn(),
    });
    await expect(controller.acceptCheckResult(available)).resolves.toMatchObject({
      phase: 'unsupported',
      availableVersion: '1.1.0',
    });
    expect(controller.apply().phase).toBe('unsupported');
  });
});
