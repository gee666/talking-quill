declare const __TALKING_QUILL_SOURCE_REVISION__: string;
declare const __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__: string;

import { app } from 'electron';
import { readFileSync } from 'node:fs';
import { basename, isAbsolute, join, resolve } from 'node:path';
import {
  authorizeInstalledAcceptanceRequest,
  consumeInstalledAcceptanceNonce,
  INSTALLED_ACCEPTANCE_ARGUMENT_FLAGS,
  INSTALLED_ACCEPTANCE_ARGUMENT_PREFIXES,
} from '../acceptance/authorization';
import type { InstalledObservationRequest } from '../acceptance/installed-observation';
import { isStrictPathChild } from '../app/runtime-path-policy';
import { startMain, type MainBootstrapOptions } from '../bootstrap';

const startup = readInstalledAcceptanceStartup(process.argv);

if (!app.isPackaged || process.platform !== 'win32') {
  throw new Error('Windows installed acceptance entry requires an authorized packaged invocation');
}
const authorization = authorizeInstalledAcceptanceRequest({
  acceptanceBuild: true,
  encodedBuildManifest: readFileSync(
    join(process.resourcesPath, 'windows-installed-acceptance-v1.txt'),
    'utf8',
  ).trim(),
  manifestPublicKeySpkiBase64url: __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__,
  sourceRevision: __TALKING_QUILL_SOURCE_REVISION__,
  encodedRequest: startup.signedRequest,
  installed: {
    resourcesPath: process.resourcesPath,
    executablePath: process.execPath,
    architecture: process.arch,
    version: app.getVersion(),
  },
});
consumeInstalledAcceptanceNonce(authorization, app.getPath('temp'));

function readInstalledAcceptanceStartup(argv: readonly string[]): { signedRequest: string } {
  const prefix = '--talking-quill-installed-acceptance-startup-pipe-v1=';
  const descriptors = argv.filter((argument) => argument.startsWith(prefix));
  const descriptor = descriptors[0];
  if (
    descriptors.length !== 1 ||
    descriptor === undefined ||
    argv.includes('--talking-quill-installed-acceptance-stdin-v1') ||
    argv.some(
      (argument) =>
        argument !== descriptor &&
        (INSTALLED_ACCEPTANCE_ARGUMENT_FLAGS.some((flag) => argument.startsWith(flag)) ||
          INSTALLED_ACCEPTANCE_ARGUMENT_PREFIXES.some((value) => argument.startsWith(value))),
    )
  ) {
    throw new Error('Installed acceptance startup descriptor is invalid');
  }
  const pipeName = descriptor.slice(prefix.length);
  if (!/^\\\\\.\\pipe\\TalkingQuill\.AcceptanceStartup\.[0-9a-f]{32}$/u.test(pipeName)) {
    throw new Error('Installed acceptance startup pipe is invalid');
  }
  const bytes = readFileSync(pipeName);
  if (bytes.length > 20 * 1024) {
    throw new Error('Installed acceptance startup frame is too large');
  }
  if (bytes.length === 0 || bytes.at(-1) !== 0x0a) {
    throw new Error('Installed acceptance startup frame is invalid');
  }
  const value = JSON.parse(bytes.subarray(0, -1).toString('utf8')) as Record<string, unknown>;
  if (
    Object.keys(value).sort().join(',') !== 'signedRequest,version' ||
    value.version !== 1 ||
    typeof value.signedRequest !== 'string' ||
    !/^[A-Za-z0-9_-]+$/u.test(value.signedRequest)
  ) {
    throw new Error('Installed acceptance startup request is invalid');
  }
  return { signedRequest: value.signedRequest };
}

let lifecycleProfile: string | undefined;
if (authorization.lifecycleUserData !== null) {
  const profile = resolve(authorization.lifecycleUserData);
  const lifecycleRoot = resolve(app.getPath('temp'), 'TalkingQuillInstalledLifecycle');
  if (
    !isAbsolute(authorization.lifecycleUserData) ||
    !isStrictPathChild(lifecycleRoot, profile) ||
    !/^(?:installed|unpacked)-[1-9][0-9]*-[0-9a-f]{16}$/u.test(basename(profile))
  ) {
    throw new Error('Installed lifecycle user-data capability is invalid');
  }
  lifecycleProfile = profile;
}

const startInstalledAcceptance = startMain as (
  options: MainBootstrapOptions & { readonly installedObservation: InstalledObservationRequest },
) => void;
startInstalledAcceptance({
  ...(lifecycleProfile === undefined ? {} : { userDataPath: lifecycleProfile }),
  hiddenStartupFailure: true,
  windowsLoginStart: authorization.command === 'login-marker',
  installedObservation: {
    command: authorization.command,
    heartbeatDurationMs: authorization.heartbeatDurationMs,
    pipeName: authorization.readinessPipe,
    launchCorrelation: authorization.launchCorrelation,
    physicalObservation: authorization.physicalObservation,
    automationValidation: authorization.automationValidation,
    automationArmedPipe: authorization.automationArmedPipe,
    automationCase: authorization.automationCase,
    expectedUserDataRoot: lifecycleProfile ?? null,
  },
});
