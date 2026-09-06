import { createHash } from 'node:crypto';
import { lstat, readFile, readdir } from 'node:fs/promises';
import { resolve } from 'node:path';
import { hostedInstallerBinding } from './windows-hosted-lifecycle-evidence.mjs';

export async function verifyHostedRuntimeTree(root, binding) {
  const names = [];
  async function walk(directory, prefix = '') {
    const directoryStat = await lstat(directory);
    if (!directoryStat.isDirectory() || directoryStat.isSymbolicLink())
      throw new Error('Runtime directory must not be a reparse point');
    for (const name of await readdir(directory)) {
      const relative = `${prefix}${name}`;
      const path = resolve(directory, name);
      const stat = await lstat(path);
      if (stat.isSymbolicLink()) throw new Error(`Runtime reparse point: ${relative}`);
      if (stat.isDirectory()) await walk(path, `${relative}/`);
      else if (stat.isFile()) names.push(relative);
      else throw new Error(`Runtime entry is not a regular file: ${relative}`);
    }
  }
  await walk(resolve(root));
  const expected = binding.files.map((file) => file.path).sort();
  if (JSON.stringify(names.sort()) !== JSON.stringify(expected))
    throw new Error('Runtime file inventory differs from the installer payload');
  for (const file of binding.files) {
    const bytes = await readFile(resolve(root, file.path));
    if (
      bytes.length !== file.size ||
      createHash('sha256').update(bytes).digest('hex') !== file.sha256
    )
      throw new Error(`Runtime file differs from installer payload: ${file.path}`);
  }
  return { fileCount: names.length };
}

export function validateHostedRuntimeEvidence(value, binding) {
  const fields = [
    'architecture',
    'sourceCommit',
    'sourceTree',
    'sourceTreeSha256',
    'version',
    'installer',
    'installerSha256',
    'bytes',
    'provenanceDocumentSha256',
  ];
  if (
    value.schemaVersion !== 2 ||
    value.kind !== 'github-hosted-native-runtime' ||
    value.result !== 'passed' ||
    fields.some((key) => value[key] !== binding[key]) ||
    !/^[1-9][0-9]*$/u.test(value.workflowRunId) ||
    value.host !== 'github-hosted' ||
    value.coverage?.installation !== 'not-exercised' ||
    value.coverage.uac !== 'not-exercised' ||
    value.coverage.installerPayload !== 'verified' ||
    value.coverage.nativeRuntime !== 'startup-observed' ||
    value.coverage.ownerAuthentication !== 'not-observed' ||
    value.coverage.transactions !== 'not-exercised' ||
    value.coverage.gracefulLifecycle !== 'not-asserted' ||
    value.runtimeFileCount !== binding.files.length ||
    value.runtimeVerifiedBefore !== true ||
    value.runtimeVerifiedAfter !== true ||
    !validProductionStartup(value.startup, binding.architecture) ||
    'lifecycle' in value ||
    'installExitCode' in value ||
    'uninstallExitCode' in value
  ) {
    throw new Error(
      'Hosted runtime evidence does not prove the exact payload and observed production startup coverage',
    );
  }
  return value;
}

function validProductionStartup(startup, architecture) {
  const positiveInteger = (value) => Number.isSafeInteger(value) && value > 0;
  const observation = startup?.observation;
  const window = observation?.window;
  const cleanup = startup?.cleanup;
  const pids = [observation?.mainPid, observation?.helper?.pid, observation?.owner?.pid];
  return (
    startup?.schemaVersion === 1 &&
    startup.kind === 'windows-production-startup-observation' &&
    startup.result === 'passed' &&
    startup.failure === null &&
    startup.architecture === architecture &&
    startup.mode === 'unpacked' &&
    Array.isArray(startup.launch?.arguments) &&
    startup.launch.arguments.length === 0 &&
    startup.launch.testHooks === false &&
    startup.launch.profile === 'fresh-hosted-default' &&
    startup.coverage?.startup === 'observed' &&
    startup.coverage.ownerAuthentication === 'not-observed' &&
    startup.coverage.transactions === 'not-exercised' &&
    startup.coverage.gracefulLifecycle === 'not-asserted' &&
    pids.every(positiveInteger) &&
    new Set(pids).size === pids.length &&
    positiveInteger(observation.stableSamples) &&
    observation.stableSamples >= 2 &&
    window?.pid === observation.mainPid &&
    window.visible === true &&
    window.title === 'Talking Quill' &&
    positiveInteger(window.width) &&
    window.width >= 400 &&
    window.width <= 4096 &&
    positiveInteger(window.height) &&
    window.height >= 300 &&
    window.height <= 4096 &&
    window.accessibilitySource === 'Windows.UIAutomation' &&
    positiveInteger(window.contentElementCount) &&
    window.contentElementCount >= 3 &&
    Array.isArray(window.markers) &&
    JSON.stringify(window.markers) === JSON.stringify(['Talking Quill', 'Welcome', 'Continue']) &&
    observation.helper.relativePath === 'resources/helper/talking-quill-helper.exe' &&
    observation.owner.relativePath === 'resources/helper/talking-quill-keyboard-owner.exe' &&
    observation.helper.parentPid === observation.mainPid &&
    observation.owner.parentPid === observation.helper.pid &&
    Array.isArray(observation.rendererPids) &&
    observation.rendererPids.length > 0 &&
    observation.rendererPids.every((pid) => positiveInteger(pid) && !pids.includes(pid)) &&
    new Set(observation.rendererPids).size === observation.rendererPids.length &&
    startup.screenshot === 'startup-window.png' &&
    positiveInteger(startup.durationMs) &&
    typeof cleanup?.closeRequested === 'boolean' &&
    typeof cleanup.forcedTermination === 'boolean' &&
    Array.isArray(cleanup.forcedPids) &&
    cleanup.forcedPids.every(positiveInteger) &&
    cleanup.forcedTermination === cleanup.forcedPids.length > 0 &&
    cleanup.remainingPackageProcesses === 0 &&
    !('error' in cleanup) &&
    !('ownerAuthenticated' in observation.owner) &&
    !('lifecycle' in startup)
  );
}

export async function verifyHostedRuntimeEvidence({
  evidencePath,
  installerPath,
  provenancePath,
  architecture,
}) {
  const binding = await hostedInstallerBinding(installerPath, provenancePath, architecture);
  return validateHostedRuntimeEvidence(JSON.parse(await readFile(evidencePath, 'utf8')), binding);
}
