import { app, dialog } from 'electron';
import { dirname, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { APP_ID, APP_NAME } from '../shared/constants/app';
import { TalkingQuillApplication } from './app/application';
import { StartupCancelledError, createFatalStartupReport } from './app/lifecycle';
import { registerPrivilegedScheme } from './security/protocol';
import { createAppPaths } from './persistence/paths';
import { resetOwnedApplicationData } from './data/data-lifecycle-service';
import { consumeUninstallResetChallenge } from './data/uninstall-reset-challenge';
import { clearLaunchAtLoginForUninstall } from './app/launch-at-login-service';
import { isStrictPathChild, selectAbsolutePathOverride } from './app/runtime-path-policy';
import { resolveHelperExecutable } from './helper';
import { createNativeOwnedTreeRemoval } from './data/native-owned-tree-removal';

registerPrivilegedScheme();
app.enableSandbox();
app.setName(APP_NAME);
app.setAppUserModelId(APP_ID);
if (process.platform === 'darwin' && !app.isPackaged) {
  app.dock?.setIcon(resolve(app.getAppPath(), 'assets', 'app-icon.png'));
}

const developmentVisibleNonce = readDevelopmentVisibleNonce();
const developmentProfile = process.env.TALKING_QUILL_DEV_VISIBLE_PROFILE;
if (developmentVisibleNonce !== null) {
  if (developmentProfile === undefined) throw new Error('Development probe profile is missing');
  const profile = resolve(developmentProfile);
  if (!isStrictPathChild(resolve(tmpdir()), profile)) {
    throw new Error('Development probe profile must be a unique temporary child');
  }
  app.setPath('appData', dirname(profile));
  app.setPath('userData', profile);
  app.on('browser-window-created', (_event, window) => {
    if (window.getTitle() !== 'Talking Quill') return;
    window.once('show', () => {
      setImmediate(() => {
        if (!window.isDestroyed() && window.isVisible()) {
          process.stdout.write(
            `TALKING_QUILL_DEV_VISIBLE:${developmentVisibleNonce}:${String(process.pid)}\n`,
          );
          app.quit();
        }
      });
    });
  });
}

const injectedUserData = readArgument('--talking-quill-user-data=');
const testUserDataOverrideAllowed =
  (!app.isPackaged && process.env.NODE_ENV === 'test') ||
  (app.isPackaged &&
    process.env.CI === 'true' &&
    process.env.TALKING_QUILL_PACKAGED_TEST === '1' &&
    process.argv.some((argument) => argument.startsWith('--remote-debugging-port=')));
if (injectedUserData !== null && testUserDataOverrideAllowed) {
  const testUserData = resolve(injectedUserData);
  app.setPath('appData', dirname(testUserData));
  app.setPath('userData', testUserData);
}
const uninstallResetChallenge = app.isPackaged
  ? readArgument('--talking-quill-reset-owned-data-and-exit=')
  : null;
const nsisEvidenceRoot = selectAbsolutePathOverride(
  process.env.TALKING_QUILL_NSIS_EVIDENCE_ROOT,
  readArgument('--talking-quill-nsis-evidence-root='),
);
let uninstallResetTarget: string | null = null;
if (uninstallResetChallenge !== null) {
  uninstallResetTarget =
    nsisEvidenceRoot === null
      ? app.getPath('userData')
      : resolve(nsisEvidenceRoot, 'profile', 'AppData', 'Roaming', 'Talking Quill');
  // Chromium opens files in userData during app readiness. Keep its transient reset-helper runtime
  // outside the owned target so Windows can atomically rename and remove the target directory.
  app.setPath(
    'userData',
    resolve(app.getPath('temp'), `talking-quill-uninstall-reset-${String(process.pid)}`),
  );
}

// Isolated packaged E2E profiles must not attach to or disturb an installed interactive instance.
const hasLock =
  (app.isPackaged && testUserDataOverrideAllowed && injectedUserData !== null) ||
  app.requestSingleInstanceLock();
let application: TalkingQuillApplication | null = null;
let restoreRequested = false;

if (!hasLock) {
  // Never reset stores while an interactive instance may still own them. The uninstaller checks
  // this nonzero exit and stops rather than claiming deletion.
  if (uninstallResetChallenge !== null) app.exit(2);
  else app.quit();
} else {
  const requestRestore = () => {
    if (application === null) restoreRequested = true;
    else application.showMain();
  };
  app.on('second-instance', requestRestore);
  app.on('activate', requestRestore);
  app.on('before-quit', (event) => application?.handleBeforeQuit(event));
  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') application?.quit();
  });

  void app
    .whenReady()
    .then(async () => {
      if (uninstallResetChallenge !== null) {
        await consumeUninstallResetChallenge(
          uninstallResetChallenge,
          app.getPath('temp'),
          process.env,
        );
        clearLaunchAtLoginForUninstall(app);
        const resetTarget = uninstallResetTarget;
        if (resetTarget === null) throw new Error('Uninstall reset target is unavailable');
        const paths = createAppPaths(resetTarget);
        if (
          (process.platform !== 'win32' && process.platform !== 'darwin') ||
          (process.arch !== 'x64' && process.arch !== 'arm64')
        ) {
          throw new Error('Uninstall reset is unavailable on this platform');
        }
        const helperExecutable = resolveHelperExecutable({
          packaged: app.isPackaged,
          resourcesPath: process.resourcesPath,
          appPath: app.getAppPath(),
          platform: process.platform,
        });
        await resetOwnedApplicationData(paths.root, {
          allowedBase: dirname(resetTarget),
          homeDirectory: app.getPath('home'),
          removeIdentityBoundDirectory: createNativeOwnedTreeRemoval(helperExecutable),
        });
        app.exit(0);
        return;
      }
      application = new TalkingQuillApplication();
      await application.start();
      if (restoreRequested) application.showMain();
    })
    .catch((error: unknown) => {
      if (error instanceof StartupCancelledError) return;
      const report = createFatalStartupReport();
      const publicDetails = {
        code: report.code,
        diagnosticId: report.diagnosticId,
      };
      if (nsisEvidenceRoot !== null) {
        console.error('Talking Quill isolated NSIS evidence failed', publicDetails, error);
      } else if (app.isPackaged || process.env.CI === 'true') {
        console.error('Talking Quill startup failed', publicDetails);
      } else {
        // Local development errors are not persisted or transmitted; retain the underlying exception
        // here so an opaque diagnostic ID does not make ABI and environment failures impossible to fix.
        console.error('Talking Quill startup failed', publicDetails, error);
      }
      dialog.showErrorBox(
        'Talking Quill could not start',
        `${report.message}\nDiagnostic ID: ${report.diagnosticId}`,
      );
      app.exit(1);
    });
}

function readDevelopmentVisibleNonce(): string | null {
  const value = process.env.TALKING_QUILL_DEV_VISIBLE_NONCE;
  if (value === undefined) return null;
  if (app.isPackaged || process.env.NODE_ENV !== 'development' || !/^[0-9a-f]{32}$/u.test(value))
    throw new Error('Development visible-window probe is unavailable');
  return value;
}

function readArgument(prefix: string): string | null {
  const argument = process.argv.find((value) => value.startsWith(prefix));
  const value = argument?.slice(prefix.length).trim();
  return value === undefined || value.length === 0 ? null : value;
}
