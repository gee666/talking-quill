declare const __TALKING_QUILL_SOURCE_REVISION__: string;
declare const __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__: string;

import { app } from 'electron';
import { readFileSync } from 'node:fs';
import { basename, isAbsolute, join, resolve } from 'node:path';
import {
  authorizeInstalledAcceptance,
  consumeInstalledAcceptanceNonce,
  hasInstalledAcceptanceArguments,
} from '../acceptance/authorization';
import type { InstalledObservationRequest } from '../acceptance/installed-observation';
import { isStrictPathChild } from '../app/runtime-path-policy';
import { startMain, type MainBootstrapOptions } from '../bootstrap';

hydrateAcceptanceArguments(process.argv);

if (
  !app.isPackaged ||
  process.platform !== 'win32' ||
  !hasInstalledAcceptanceArguments(process.argv)
) {
  throw new Error('Windows installed acceptance entry requires an authorized packaged invocation');
}
const authorization = authorizeInstalledAcceptance({
  acceptanceBuild: true,
  encodedBuildManifest: readFileSync(
    join(process.resourcesPath, 'windows-installed-acceptance-v1.txt'),
    'utf8',
  ).trim(),
  manifestPublicKeySpkiBase64url: __TALKING_QUILL_ACCEPTANCE_MANIFEST_PUBLIC_KEY_SPKI_BASE64URL__,
  sourceRevision: __TALKING_QUILL_SOURCE_REVISION__,
  argv: process.argv,
  installed: {
    resourcesPath: process.resourcesPath,
    executablePath: process.execPath,
    architecture: process.arch,
    version: app.getVersion(),
  },
});
consumeInstalledAcceptanceNonce(authorization, app.getPath('temp'));

function hydrateAcceptanceArguments(argv: string[]): void {
  const marker = '--talking-quill-installed-acceptance-fd=3';
  const matches = argv.filter((argument) => argument === marker);
  if (matches.length === 0) return;
  if (matches.length !== 1) throw new Error('Installed acceptance startup descriptor is invalid');
  const bytes = readFileSync(3);
  if (bytes.length === 0 || bytes.length > 20 * 1024 || bytes.at(-1) !== 0x0a) {
    throw new Error('Installed acceptance startup frame is invalid');
  }
  const value = JSON.parse(bytes.subarray(0, -1).toString('utf8')) as {
    version?: unknown;
    arguments?: unknown;
  };
  if (
    value.version !== 1 ||
    !Array.isArray(value.arguments) ||
    value.arguments.length === 0 ||
    value.arguments.some((argument) => typeof argument !== 'string' || argument.length > 18_000)
  ) {
    throw new Error('Installed acceptance startup arguments are invalid');
  }
  for (const argument of value.arguments) argv.push(argument as string);
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
