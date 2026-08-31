import { createHash } from 'node:crypto';
import { spawn } from 'node:child_process';
import { createReadStream } from 'node:fs';
import { basename } from 'node:path';
import { app } from 'electron';
import { autoUpdater } from 'electron-updater';
import type { ApplicationUpdateBackend } from './application-update-controller';
import { resolveHelperExecutable } from '../helper/helper-path';
import { parseUnsignedUpdateIdentity } from './unsigned-update-identity';
import { PublicationCatalog } from './publication-catalog';
import type { VerifiedPublication } from './publication-catalog';
import { signedPublicationProviderOptions } from './signed-publication-provider';
import { readInstalledWindowsUpdateIdentity } from './installed-windows-update-identity';
import { buildWindowsElevationLaunch, settleWindowsElevation } from './windows-update-launch';
import {
  clearWindowsUpdateRelaunchIntent,
  createWindowsUpdateRelaunchIntent,
  wrapWindowsUpdateRelaunchRequest,
} from './windows-update-relaunch-intent';

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
  autoUpdater.disableDifferentialDownload = true;
  autoUpdater.channel = `latest-${architecture}`;
  // Setting a custom channel enables downgrades in electron-updater; stable releases never do that.
  autoUpdater.allowDowngrade = false;

  const publicationCatalog = new PublicationCatalog();
  let selectedPublication: VerifiedPublication | null = null;
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
      checkedIdentity = null;
      selectedPublication = null;
      const selected =
        process.platform === 'win32'
          ? await publicationCatalog.select(
              architecture,
              await readInstalledWindowsUpdateIdentity(process.resourcesPath, architecture),
            )
          : null;
      if (selected !== null) {
        autoUpdater.setFeedURL(signedPublicationProviderOptions(selected));
      }
      const result = await autoUpdater.checkForUpdates();
      if (result === null) return null;
      if (selected !== null && result.updateInfo.version !== selected.version) {
        throw new Error('Updater metadata does not match the selected signed publication');
      }
      const identity = parseUnsignedUpdateIdentity(
        (result.updateInfo as unknown as { talkingQuillRelease?: unknown }).talkingQuillRelease,
        process.platform,
        architecture,
        result.updateInfo.version,
      );
      if (selected !== null && identity.packageSha256 !== selected.packageSha256) {
        throw new Error('Updater identity does not match the selected signed publication object');
      }
      checkedIdentity = identity;
      selectedPublication = selected;
      return {
        version: result.updateInfo.version,
        releaseUrl: selected?.release.html_url ?? null,
      };
    },
    async downloadUpdate() {
      const identity = checkedIdentity;
      const publication = selectedPublication;
      if (identity === null || (process.platform === 'win32' && publication === null))
        throw new Error('Signed updater release identity is unavailable');
      const files = await autoUpdater.downloadUpdate();
      const expectedName =
        process.platform === 'win32'
          ? (publication?.packageAsset.name ?? '')
          : basename(files.find((file) => file.toLowerCase().endsWith('.zip')) ?? '');
      const payload = files.find((file) => basename(file) === expectedName);
      if (payload === undefined) throw new Error('Downloaded update payload is missing');
      const payloadSha256 = await sha256File(payload);
      if (
        payloadSha256 !== identity.packageSha256 ||
        (process.platform === 'win32' && payloadSha256 !== publication?.packageSha256)
      ) {
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
      const relaunch = await createWindowsUpdateRelaunchIntent(
        app.getPath('userData'),
        app.getVersion(),
        checkedIdentity.version,
      );
      const argument = launch.arguments.at(0);
      if (argument === undefined)
        throw new Error('Windows update bootstrap request is unavailable');
      const accepted = await launchWindowsElevation(launch.executable, [
        wrapWindowsUpdateRelaunchRequest(argument, relaunch.path, relaunch.intent.nonce),
      ]);
      windowsElevationAccepted = accepted;
      if (!accepted) await clearWindowsUpdateRelaunchIntent(relaunch.path);
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
      checkedIdentity = null;
      selectedPublication = null;
      downloadedWindowsInstaller = null;
      autoUpdater.removeListener('download-progress', handleProgress);
      autoUpdater.removeListener('error', handleError);
      progressListeners.clear();
      errorListeners.clear();
    },
  };
}

export async function launchWindowsUpdateReadyHelper(
  executable: string,
  argument: string,
): Promise<void> {
  await new Promise<void>((resolveReady, rejectReady) => {
    const child = spawn(executable, [argument], { stdio: 'ignore', windowsHide: true });
    child.once('error', rejectReady);
    child.once('exit', (code) => {
      if (code === 0) resolveReady();
      else rejectReady(new Error('Native update readiness acknowledgement failed'));
    });
  });
}

async function launchWindowsElevation(
  executable: string,
  arguments_: readonly string[],
): Promise<boolean> {
  return await new Promise<boolean>((resolveLaunch, rejectLaunch) => {
    const child = spawn(executable, arguments_, { stdio: 'ignore', windowsHide: true });
    let settled = false;
    const settle = (accepted: boolean): void => {
      if (settled) return;
      settled = true;
      resolveLaunch(accepted);
    };
    const fail = (): void => {
      if (settled) return;
      settled = true;
      rejectLaunch(new Error('The Windows update bootstrap failed'));
    };
    child.once('error', fail);
    child.once('exit', (code) =>
      settleWindowsElevation(
        code,
        () => settle(true),
        () => settle(false),
        fail,
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
