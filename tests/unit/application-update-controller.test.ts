import { describe, expect, it, vi } from 'vitest';
import {
  ApplicationUpdateController,
  type ApplicationUpdateBackend,
} from '../../app/src/main/info/application-update-controller';

function backend(version = '1.1.0') {
  let progress: ((percent: number) => void) | null = null;
  let error: (() => void) | null = null;
  const checkForUpdates = vi.fn(() => Promise.resolve({ version }));
  const downloadUpdate = vi.fn(() => Promise.resolve());
  const quitAndInstall = vi.fn();
  const value: ApplicationUpdateBackend = {
    checkForUpdates,
    downloadUpdate,
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
  it('downloads only after consent and requests a drained install', async () => {
    const updater = backend();
    const publish = vi.fn();
    const requestInstall = vi.fn();
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: updater.value,
      publish,
      requestInstall,
    });

    expect(controller.acceptCheckResult(available)).toMatchObject({
      phase: 'available',
      availableVersion: '1.1.0',
    });
    expect(updater.checkForUpdates).not.toHaveBeenCalled();
    expect(controller.apply().phase).toBe('downloading');
    updater.progress(52.5);
    expect(controller.getState()).toMatchObject({ phase: 'downloading', percent: 52.5 });
    await vi.waitFor(() => expect(requestInstall).toHaveBeenCalledOnce());
    expect(controller.getState()).toMatchObject({ phase: 'installing', percent: 100 });
    controller.quitAndInstall();
    expect(updater.quitAndInstall).toHaveBeenCalledOnce();
    expect(publish).toHaveBeenCalled();
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
    controller.acceptCheckResult(available);
    controller.apply();
    await vi.waitFor(() => expect(controller.getState().phase).toBe('error'));
    expect(updater.downloadUpdate).not.toHaveBeenCalled();
    expect(requestInstall).not.toHaveBeenCalled();
  });

  it('never claims automatic installation without a packaged updater backend', () => {
    const controller = new ApplicationUpdateController({
      currentVersion: '1.0.0',
      backend: null,
      publish: vi.fn(),
      requestInstall: vi.fn(),
    });
    expect(controller.acceptCheckResult(available)).toMatchObject({
      phase: 'unsupported',
      availableVersion: '1.1.0',
    });
    expect(controller.apply().phase).toBe('unsupported');
  });
});
