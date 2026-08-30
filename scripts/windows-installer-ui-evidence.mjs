import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';

const HEX_40 = /^[0-9a-f]{40}$/u;
const HEX_64 = /^[0-9a-f]{64}$/u;
export const WINDOWS_INSTALLER_UI_CANCELLATION_EXIT_CODE = 0;

export function validateWindowsInstallerUiEvidence(value, expected) {
  const monitoring = value?.monitoring;
  const cancellation = value?.cancellation;
  const window = value?.nsisWindow;
  const nsisRoles = ['outer', 'elevated', 'protected'];
  const rolePids = nsisRoles.map((role) => value?.nsisRoleExits?.[role]?.pid);
  const roleExitsMatchProcesses = nsisRoles.every((role) => {
    const expectedRole = value?.nsisRoleExits?.[role];
    return value?.processes?.some(
      (process) =>
        process?.pid === expectedRole?.pid && process?.role === role && process?.exitCode === 0,
    );
  });
  if (
    value?.schemaVersion !== 2 ||
    value.installer !== expected.installer ||
    value.architecture !== expected.architecture ||
    value.sourceCommit !== expected.sourceCommit ||
    value.sourceTree !== expected.sourceTree ||
    value.sourceTreeSha256 !== expected.sourceTreeSha256 ||
    value.installerProvenanceSha256 !== expected.installerSha256 ||
    value.provenanceDocumentSha256 !== expected.provenanceDocumentSha256 ||
    value.installerSha256Before !== expected.installerSha256 ||
    value.installerSha256After !== expected.installerSha256 ||
    value.bytes !== expected.bytes ||
    value.outerPeSubsystem !== 'windows-gui' ||
    value.outerPeSubsystemValue !== 2 ||
    window?.className !== '#32770' ||
    typeof window.title !== 'string' ||
    !window.title.toLowerCase().includes('talking quill') ||
    !Number.isSafeInteger(window.processId) ||
    window.processId < 1 ||
    monitoring?.sampleIntervalMs !== 5 ||
    !Number.isSafeInteger(monitoring.maximumSampleGapMs) ||
    monitoring.maximumSampleGapMs > 50 ||
    monitoring.maximumSampleGapMs < 0 ||
    !['processSamples', 'windowSamples', 'filesystemSamples', 'registrySamples'].every(
      (name) => Number.isSafeInteger(monitoring[name]) && monitoring[name] >= 2,
    ) ||
    !Array.isArray(monitoring.errors) ||
    monitoring.errors.length !== 0 ||
    !Array.isArray(monitoring.events) ||
    monitoring.events.length === 0 ||
    !Array.isArray(value.visibleConsoleWindowEvents) ||
    value.visibleConsoleWindowEvents.length !== 0 ||
    !Array.isArray(value.filesystemOrRegistryMutationEvents) ||
    value.filesystemOrRegistryMutationEvents.length !== 0 ||
    !Array.isArray(value.processes) ||
    value.processes.length < 3 ||
    !nsisRoles.every(
      (role) =>
        value.nsisRoleExits?.[role]?.exitCode === 0 &&
        Number.isSafeInteger(value.nsisRoleExits[role].pid) &&
        value.nsisRoleExits[role].pid > 0,
    ) ||
    new Set(rolePids).size !== nsisRoles.length ||
    !roleExitsMatchProcesses ||
    window?.processId !== value.nsisRoleExits.protected.pid ||
    !Number.isSafeInteger(value.powershellProcessStarts) ||
    value.powershellProcessStarts < 2 ||
    value.transientProtectedBootstrapObserved !== true ||
    value.protectedBootstrapBaselineRestored !== true ||
    cancellation?.method !== 'WM_COMMAND/IDCANCEL' ||
    cancellation.targetRole !== 'protected' ||
    cancellation.targetProcessId !== window.processId ||
    cancellation.postAccepted !== true ||
    typeof cancellation.confirmationObserved !== 'boolean' ||
    typeof cancellation.confirmationPostAccepted !== 'boolean' ||
    (cancellation.confirmationObserved &&
      (cancellation.confirmationProcessId !== window.processId ||
        cancellation.confirmationPostAccepted !== true)) ||
    cancellation.graceful !== true ||
    cancellation.forcedCleanup !== false ||
    cancellation.exitCode !== WINDOWS_INSTALLER_UI_CANCELLATION_EXIT_CODE ||
    !Array.isArray(value.activeProcessesAfterTeardown) ||
    value.activeProcessesAfterTeardown.length !== 0 ||
    value.noDurableInstallMutation !== true ||
    value.exactBaselineRestored !== true ||
    value.passed !== true
  ) {
    throw new Error('Windows installer UI smoke evidence is not promotable');
  }
  return value;
}

export async function verifyWindowsInstallerUiEvidence({
  evidencePath,
  installerPath,
  provenancePath,
  architecture,
}) {
  const [evidenceSource, installerBytes, provenanceSource] = await Promise.all([
    readFile(evidencePath, 'utf8'),
    readFile(installerPath),
    readFile(provenancePath),
  ]);
  const provenance = JSON.parse(provenanceSource.toString('utf8'));
  validateArtifactProvenanceManifest(provenance);
  const installerSha256 = createHash('sha256').update(installerBytes).digest('hex');
  const provenanceDocumentSha256 = createHash('sha256').update(provenanceSource).digest('hex');
  const finalEntries = provenance.entries.filter(({ role }) => role === 'final-artifact');
  if (
    !['x64', 'arm64'].includes(architecture) ||
    provenance.package.platform !== 'win' ||
    provenance.package.arch !== architecture ||
    !HEX_40.test(provenance.sourceCommit) ||
    !HEX_40.test(provenance.sourceTree) ||
    !HEX_64.test(provenance.sourceTreeSha256) ||
    finalEntries.length !== 1 ||
    basename(finalEntries[0].path) !== basename(installerPath) ||
    !HEX_64.test(finalEntries[0].sha256) ||
    finalEntries[0].sha256 !== installerSha256
  ) {
    throw new Error('Windows installer UI smoke does not match exact artifact provenance');
  }
  return validateWindowsInstallerUiEvidence(JSON.parse(evidenceSource), {
    installer: basename(installerPath),
    architecture,
    sourceCommit: provenance.sourceCommit,
    sourceTree: provenance.sourceTree,
    sourceTreeSha256: provenance.sourceTreeSha256,
    installerSha256,
    provenanceDocumentSha256,
    bytes: installerBytes.length,
  });
}

function argument(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  const evidencePath = argument('--evidence');
  const installerPath = argument('--installer');
  const provenancePath = argument('--provenance');
  const architecture = argument('--arch');
  if ([evidencePath, installerPath, provenancePath, architecture].some((value) => !value)) {
    throw new Error(
      'Usage: windows-installer-ui-evidence --evidence <json> --installer <exe> --provenance <json> --arch <x64|arm64>',
    );
  }
  await verifyWindowsInstallerUiEvidence({
    evidencePath: resolve(evidencePath),
    installerPath: resolve(installerPath),
    provenancePath: resolve(provenancePath),
    architecture,
  });
  console.log(`Windows ${architecture} installer UI smoke evidence verified.`);
}
