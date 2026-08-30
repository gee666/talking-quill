declare const __TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD__: boolean;
declare const __TALKING_QUILL_SOURCE_REVISION__: string;
declare const __TALKING_QUILL_ACCEPTANCE_BUILD__: boolean;
declare const __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__: string;

import { app, dialog } from 'electron';
import { readFileSync } from 'node:fs';
import { createRequire } from 'node:module';
import { basename, dirname, isAbsolute, join, resolve } from 'node:path';
import { tmpdir } from 'node:os';
import { APP_ID, APP_NAME } from '../shared/constants/app';
import { TalkingQuillApplication } from './app/application';
import { classifyWindowsLoginStartArguments } from './app/launch-at-login-service';
import { StartupCancelledError, createFatalStartupReport } from './app/lifecycle';
import { resolveSignedInWindowsUserDataTarget } from './app/windows-uninstall-target';
import { registerPrivilegedScheme } from './security/protocol';
import { createAppPaths } from './persistence/paths';
import { resetOwnedApplicationData } from './data/data-lifecycle-service';
import { consumeUninstallResetChallenge } from './data/uninstall-reset-challenge';
import { isStrictPathChild, validateUninstallResetTarget } from './app/runtime-path-policy';
import { resolveOwnedTreeRemovalExecutable } from './helper';
import { createNativeOwnedTreeRemoval } from './data/native-owned-tree-removal';
import {
  authorizeInstalledAcceptance,
  consumeInstalledAcceptanceNonce,
  hasInstalledAcceptanceArguments,
} from './security/acceptance-authorization';

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

const windowsLoginStartClassification = classifyWindowsLoginStartArguments(
  process.argv,
  app.isPackaged,
  process.platform,
);
if (windowsLoginStartClassification === 'invalid') {
  throw new Error('Windows login-start argument is invalid for this launch');
}
const windowsLoginStart = windowsLoginStartClassification === 'login-start';

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
const acceptanceAuthorization =
  app.isPackaged && hasInstalledAcceptanceArguments(process.argv)
    ? authorizeInstalledAcceptance({
        acceptanceBuild: process.platform === 'win32' && __TALKING_QUILL_ACCEPTANCE_BUILD__,
        encodedBuildManifest: readFileSync(
          join(process.resourcesPath, 'windows-installed-acceptance-v1.txt'),
          'utf8',
        ).trim(),
        manifestPublicKeySpkiBase64url:
          __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__,
        sourceRevision: __TALKING_QUILL_SOURCE_REVISION__,
        argv: process.argv,
        installed: {
          resourcesPath: process.resourcesPath,
          executablePath: process.execPath,
          architecture: process.arch,
          version: app.getVersion(),
        },
      })
    : null;
if (acceptanceAuthorization !== null) {
  consumeInstalledAcceptanceNonce(acceptanceAuthorization, app.getPath('temp'));
}
const installedReadinessPipe = acceptanceAuthorization?.readinessPipe ?? null;
const installedAutomationArmedPipe = acceptanceAuthorization?.automationArmedPipe ?? null;
const installedLaunchCorrelation = acceptanceAuthorization?.launchCorrelation ?? null;
const installedPhysicalObservation = acceptanceAuthorization?.physicalObservation ?? false;
const installedAutomationValidation = acceptanceAuthorization?.automationValidation ?? false;
const installedAutomationCase = acceptanceAuthorization?.automationCase ?? null;
let installedLifecycleProfile: string | null = null;
const installedLifecycleUserData = acceptanceAuthorization?.lifecycleUserData ?? null;
if (installedLifecycleUserData !== null) {
  const profile = resolve(installedLifecycleUserData);
  const lifecycleRoot = resolve(app.getPath('temp'), 'TalkingQuillInstalledLifecycle');
  if (
    process.platform !== 'win32' ||
    installedReadinessPipe === null ||
    !isAbsolute(installedLifecycleUserData) ||
    !isStrictPathChild(lifecycleRoot, profile) ||
    !/^(?:installed|unpacked)-[1-9][0-9]*-[0-9a-f]{16}$/u.test(basename(profile))
  ) {
    throw new Error('Installed lifecycle user-data capability is invalid');
  }
  installedLifecycleProfile = profile;
  app.setPath('appData', dirname(profile));
  app.setPath('userData', profile);
}
const uninstallResetChallenge = app.isPackaged
  ? readArgument('--talking-quill-reset-owned-data-and-exit=')
  : null;
const uninstallResetTargetArgument = process.env.TALKING_QUILL_UNINSTALL_RESET_TARGET;
let uninstallResetTarget: string | null = null;
if (uninstallResetChallenge !== null) {
  const isolatedUninstallTest =
    __TALKING_QUILL_UNINSTALL_ISOLATED_VALIDATION_BUILD__ &&
    process.env.TALKING_QUILL_UNINSTALL_ISOLATED_TEST === '1';
  if (uninstallResetTargetArgument === undefined) {
    throw new Error('Uninstall reset target transfer is unavailable');
  }
  uninstallResetTarget = validateUninstallResetTarget(
    uninstallResetTargetArgument,
    isolatedUninstallTest
      ? { isolatedTestBase: resolve(app.getPath('temp'), 'TQTests') }
      : { expectedTarget: resolveSignedInWindowsUserDataTarget() },
  );
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
let restoreRequested: 'second_instance' | 'os_activate' | null = null;
let machineQuitRequested = process.argv.includes('--talking-quill-request-machine-quit');

if (!hasLock) {
  // Never reset stores while an interactive instance may still own them. The uninstaller checks
  // this nonzero exit and stops rather than claiming deletion.
  if (uninstallResetChallenge !== null) app.exit(2);
  else app.quit();
} else {
  const requestRestore = (source: 'second_instance' | 'os_activate') => {
    if (application === null) restoreRequested = source;
    else application.handleApplicationActivation(source);
  };
  app.on('second-instance', (_event, commandLine) => {
    const loginStart = classifyWindowsLoginStartArguments(
      commandLine,
      app.isPackaged,
      process.platform,
    );
    // Login registration may race an already running instance. A valid marker
    // is background-only and is observable only by an authorized acceptance run.
    if (loginStart === 'login-start') {
      application?.handleAcceptanceLoginStart();
      return;
    }
    if (loginStart === 'invalid') return;
    if (commandLine.includes('--talking-quill-request-machine-quit')) {
      machineQuitRequested = true;
      if (application === null) app.quit();
      else application.quit();
      return;
    }
    requestRestore('second_instance');
  });
  app.on('activate', () => requestRestore('os_activate'));
  app.on('before-quit', (event) => application?.handleBeforeQuit(event));
  app.on('window-all-closed', () => {
    if (process.platform !== 'darwin') application?.quit();
  });

  void app
    .whenReady()
    .then(async () => {
      if (
        app.isPackaged &&
        process.platform === 'darwin' &&
        process.argv.includes('--macos-owner-acl-denial-test')
      ) {
        // N-API runs SecItemCopyMatching inside this exact packaged Electron
        // process. A helper/bridge subprocess would test the wrong code identity.
        const loadNative = createRequire(import.meta.url);
        const observation: unknown = loadNative(
          join(process.resourcesPath, 'macos-keychain-denial.node'),
        ) as unknown;
        const expectedDenials = new Set([-25_308, -25_293]); // interactionNotAllowed/authFailed
        app.exit(typeof observation === 'number' && expectedDenials.has(observation) ? 77 : 1);
        return;
      }
      if (uninstallResetChallenge !== null) {
        await consumeUninstallResetChallenge(
          uninstallResetChallenge,
          app.getPath('temp'),
          process.env,
        );
        const resetTarget = uninstallResetTarget;
        if (resetTarget === null) throw new Error('Uninstall reset target is unavailable');
        const paths = createAppPaths(resetTarget);
        if (
          (process.platform !== 'win32' && process.platform !== 'darwin') ||
          (process.arch !== 'x64' && process.arch !== 'arm64')
        ) {
          throw new Error('Uninstall reset is unavailable on this platform');
        }
        const helperExecutable = resolveOwnedTreeRemovalExecutable({
          packaged: app.isPackaged,
          resourcesPath: process.resourcesPath,
          appPath: app.getAppPath(),
          platform: process.platform,
        });
        const resetOptions = {
          // Bind canonical containment to the profile root, not Roaming. A
          // swapped AppData or Roaming junction must resolve outside this base.
          allowedBase: dirname(dirname(dirname(resetTarget))),
          homeDirectory: app.getPath('home'),
          removeIdentityBoundDirectory: createNativeOwnedTreeRemoval(helperExecutable),
        };
        try {
          await resetOwnedApplicationData(paths.root, resetOptions);
        } catch {
          // The native remover may finish as bounded process observation fails.
          // Resume the identity-bound journal once before reporting failure.
          await resetOwnedApplicationData(paths.root, resetOptions);
        }
        app.exit(0);
        return;
      }
      application = new TalkingQuillApplication({ windowsLoginStart });
      await application.start();
      if (machineQuitRequested) {
        application.quit();
        return;
      }
      if (installedReadinessPipe !== null) {
        if (installedLaunchCorrelation === null || acceptanceAuthorization === null) {
          throw new Error('Installed readiness authorization was not retained');
        }
        try {
          await application.runInstalledObservation({
            command: acceptanceAuthorization.command,
            heartbeatDurationMs: acceptanceAuthorization.heartbeatDurationMs,
            pipeName: installedReadinessPipe,
            launchCorrelation: installedLaunchCorrelation,
            physicalObservation: installedPhysicalObservation,
            automationValidation: installedAutomationValidation,
            automationArmedPipe: installedAutomationArmedPipe,
            automationCase: installedAutomationCase,
            expectedUserDataRoot: installedLifecycleProfile,
          });
        } finally {
          await application.stop();
        }
      }
      if (restoreRequested !== null) application.handleApplicationActivation(restoreRequested);
    })
    .catch((error: unknown) => {
      if (error instanceof StartupCancelledError) return;
      const report = createFatalStartupReport();
      if (uninstallResetChallenge !== null) {
        app.exit(1);
        return;
      }
      if (installedReadinessPipe !== null) {
        // Installed readiness is a hidden, noninteractive diagnostic. Never
        // strand it behind a modal dialog; the invoking validator owns bounded
        // cleanup of its isolated temporary Chromium profile after this exit.
        app.exit(1);
        return;
      }
      const publicDetails = {
        code: report.code,
        diagnosticId: report.diagnosticId,
      };
      if (app.isPackaged || process.env.CI === 'true') {
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
