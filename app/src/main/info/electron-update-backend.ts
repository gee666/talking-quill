import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { app } from 'electron';
import { autoUpdater } from 'electron-updater';
import type { ApplicationUpdateBackend } from './application-update-controller';
import { resolveHelperExecutable } from '../helper/helper-path';
import { parseUnsignedUpdateIdentity } from './unsigned-update-identity';
import { buildWindowsElevationLaunch, settleWindowsElevation } from './windows-update-launch';

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

  let downloadedWindowsInstaller: {
    readonly path: string;
    readonly sha256: string;
  } | null = null;
  let checkedIdentity: ReturnType<typeof parseUnsignedUpdateIdentity> | null = null;
  let windowsElevationAccepted = false;
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
      if (result === null) return null;
      checkedIdentity = parseUnsignedUpdateIdentity(
        (result.updateInfo as unknown as { talkingQuillRelease?: unknown }).talkingQuillRelease,
        process.platform,
        architecture,
        result.updateInfo.version,
      );
      return { version: result.updateInfo.version };
    },
    async downloadUpdate() {
      const identity = checkedIdentity;
      if (identity === null) throw new Error('Unsigned updater release identity is unavailable');
      const files = await autoUpdater.downloadUpdate();
      const payload = files.find((file) =>
        file.toLowerCase().endsWith(process.platform === 'win32' ? '.exe' : '.zip'),
      );
      if (payload === undefined) throw new Error('Downloaded update payload is missing');
      const payloadSha256 = await sha256File(payload);
      if (payloadSha256 !== identity.packageSha256) {
        throw new Error('Downloaded update payload does not match its SHA-256 identity');
      }
      if (process.platform === 'win32') {
        downloadedWindowsInstaller = {
          path: payload,
          sha256: payloadSha256,
        };
      }
      return { files, identity };
    },
    async requestElevation() {
      if (process.platform !== 'win32') return 'accepted';
      if (downloadedWindowsInstaller === null || checkedIdentity === null) {
        throw new Error('Downloaded Windows installer identity is unavailable');
      }
      if (!app.isPackaged)
        throw new Error('Windows elevated updates require the installed bootstrap');
      const bootstrap = resolveHelperExecutable({
        packaged: true,
        resourcesPath: process.resourcesPath,
        appPath: app.getAppPath(),
        platform: 'win32',
      });
      if (
        checkedIdentity.platform !== 'win' ||
        checkedIdentity.predecessor === null ||
        checkedIdentity.authorization === undefined
      ) {
        throw new Error('Windows update authorization or predecessor identity is unavailable');
      }
      const launch = buildWindowsElevationLaunch(
        process.env.SystemRoot ?? 'C:\\Windows',
        bootstrap,
        downloadedWindowsInstaller.path,
        downloadedWindowsInstaller.sha256,
        {
          ...checkedIdentity,
          platform: 'win',
          authorization: checkedIdentity.authorization,
          predecessor: { ...checkedIdentity.predecessor, platform: 'win' },
        },
      );
      const accepted = await launchWindowsElevation(launch.executable, launch.arguments);
      windowsElevationAccepted = accepted;
      return accepted ? 'accepted' : 'cancelled';
    },
    quitAndInstall() {
      if (process.platform === 'win32') {
        if (!windowsElevationAccepted) throw new Error('Windows elevation was not accepted');
        app.quit();
        return;
      }
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

async function launchWindowsElevation(
  executable: string,
  arguments_: readonly string[],
): Promise<boolean> {
  return await new Promise<boolean>((resolveLaunch) => {
    const child = spawn(executable, arguments_, { stdio: 'ignore', windowsHide: true });
    let settled = false;
    const settle = (accepted: boolean): void => {
      if (settled) return;
      settled = true;
      resolveLaunch(accepted);
    };
    child.once('error', () => settle(false));
    child.once('exit', (code) =>
      settleWindowsElevation(
        code,
        () => settle(true),
        () => settle(false),
      ),
    );
  });
}

async function sha256File(path: string): Promise<string> {
  const hash = createHash('sha256');
  await new Promise<void>((resolveHash, reject) => {
    const stream = createReadStream(path);
    stream.on('data', (chunk) => {
      if (typeof chunk === 'string') hash.update(chunk, 'utf8');
      else hash.update(chunk);
    });
    stream.once('end', resolveHash);
    stream.once('error', reject);
  });
  return hash.digest('hex');
}
