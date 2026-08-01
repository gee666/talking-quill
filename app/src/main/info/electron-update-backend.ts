import { autoUpdater } from 'electron-updater';
import type { ApplicationUpdateBackend } from './application-update-controller';

export function createElectronUpdateBackend(
  architecture: NodeJS.Architecture,
): ApplicationUpdateBackend {
  if (architecture !== 'x64' && architecture !== 'arm64') {
    throw new Error(`Unsupported update architecture: ${architecture}`);
  }
  autoUpdater.logger = null;
  autoUpdater.autoDownload = false;
  autoUpdater.autoInstallOnAppQuit = false;
  autoUpdater.autoRunAppAfterInstall = true;
  autoUpdater.allowPrerelease = false;
  autoUpdater.disableWebInstaller = true;
  autoUpdater.channel = `latest-${architecture}`;
  // Setting a custom channel enables downgrades in electron-updater; stable releases never do that.
  autoUpdater.allowDowngrade = false;

  const progressListeners = new Set<(percent: number) => void>();
  const errorListeners = new Set<() => void>();
  const handleProgress = (progress: { readonly percent: number }): void => {
    for (const listener of progressListeners) listener(progress.percent);
  };
  const handleError = (): void => {
    for (const listener of errorListeners) listener();
  };
  autoUpdater.on('download-progress', handleProgress);
  autoUpdater.on('error', handleError);

  return {
    async checkForUpdates() {
      const result = await autoUpdater.checkForUpdates();
      return result === null ? null : { version: result.updateInfo.version };
    },
    async downloadUpdate() {
      await autoUpdater.downloadUpdate();
    },
    quitAndInstall() {
      autoUpdater.quitAndInstall(true, true);
    },
    onProgress(listener) {
      progressListeners.add(listener);
      return () => progressListeners.delete(listener);
    },
    onError(listener) {
      errorListeners.add(listener);
      return () => errorListeners.delete(listener);
    },
    dispose() {
      autoUpdater.removeListener('download-progress', handleProgress);
      autoUpdater.removeListener('error', handleError);
      progressListeners.clear();
      errorListeners.clear();
    },
  };
}
