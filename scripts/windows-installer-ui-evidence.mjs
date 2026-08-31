import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';

const HEX_40 = /^[0-9a-f]{40}$/u;
const HEX_64 = /^[0-9a-f]{64}$/u;

export function validateWindowsInstallerUiEvidence(value, expected) {
  const roles = ['medium-controller', 'elevated-worker'];
  const exits = value?.installerRoleExits;
  const pids = roles.map((role) => exits?.[role]?.pid);
  const pipe = value?.authenticatedPipe;
  const manifest = value?.packageManifest;
  if (
    value?.schemaVersion !== 3 ||
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
    value.setupWindow?.processId !== exits?.['medium-controller']?.pid ||
    value.setupWindow?.className !== '#32770' ||
    !roles.every(
      (role) =>
        Number.isSafeInteger(exits?.[role]?.pid) &&
        exits[role].pid > 0 &&
        exits[role].exitCode === 0,
    ) ||
    new Set(pids).size !== 2 ||
    !roles.every((role) =>
      value.processes?.some(
        (process) =>
          process.pid === exits[role].pid &&
          process.role === role &&
          process.consoleWindow === false,
      ),
    ) ||
    pipe?.oneShot !== true ||
    pipe?.controllerPid !== exits['medium-controller'].pid ||
    pipe?.workerPid !== exits['elevated-worker'].pid ||
    pipe?.clientProcessIdVerified !== true ||
    pipe?.serverProcessIdVerified !== true ||
    pipe?.sameImageSha256Verified !== true ||
    pipe?.challengeProofVerified !== true ||
    manifest?.magic !== 'TQPKG2' ||
    manifest?.canonical !== true ||
    manifest?.fullTreeVerified !== true ||
    manifest?.architecture !== expected.architecture ||
    value.powershellProcessStarts !== 0 ||
    value.interpreterProcessStarts !== 0 ||
    value.successfulDefaultLifecycle !== true ||
    value.forcedCleanup !== false ||
    value.authoritativeZeroResidue !== true ||
    !Array.isArray(value.activeProcessesAfterTeardown) ||
    value.activeProcessesAfterTeardown.length !== 0 ||
    !Array.isArray(value.residueAfterTeardown) ||
    value.residueAfterTeardown.length !== 0 ||
    value.passed !== true
  ) {
    throw new Error('Windows native setup UI evidence is not promotable');
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
    finalEntries[0].sha256 !== installerSha256
  ) {
    throw new Error('Windows native setup UI smoke does not match exact artifact provenance');
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
  if ([evidencePath, installerPath, provenancePath, architecture].some((value) => !value))
    throw new Error(
      'Usage: windows-installer-ui-evidence --evidence <json> --installer <exe> --provenance <json> --arch <x64|arm64>',
    );
  await verifyWindowsInstallerUiEvidence({
    evidencePath: resolve(evidencePath),
    installerPath: resolve(installerPath),
    provenancePath: resolve(provenancePath),
    architecture,
  });
}
