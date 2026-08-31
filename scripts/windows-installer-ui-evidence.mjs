import { createHash } from 'node:crypto';
import { readFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';
import { parseTqpkg2 } from './tqpkg2.mjs';

const HEX_40 = /^[0-9a-f]{40}$/u;
const HEX_64 = /^[0-9a-f]{64}$/u;

export function validateWindowsInstallerUiEvidence(value, expected) {
  const cancel = value?.cancel;
  const residueBefore = Array.isArray(value?.residueBefore)
    ? [...value.residueBefore].sort()
    : null;
  const residueAfter = Array.isArray(value?.residueAfter) ? [...value.residueAfter].sort() : null;
  const computedNewResidue =
    residueBefore !== null && residueAfter !== null
      ? residueAfter.filter((entry) => !residueBefore.includes(entry))
      : null;
  if (
    value?.schemaVersion !== 4 ||
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
    !Number.isSafeInteger(value.setupWindow?.processId) ||
    value.setupWindow.processId <= 0 ||
    value.setupWindow?.className !== '#32770' ||
    cancel?.commandSent !== true ||
    cancel?.exitCode !== 1223 ||
    cancel?.workerStarted !== false ||
    cancel?.pipeObserved !== false ||
    !Array.isArray(value.processStarts) ||
    !Array.isArray(value.interpreterProcessStarts) ||
    value.interpreterProcessStarts.length !== 0 ||
    !Array.isArray(value.observerErrors) ||
    value.observerErrors.length !== 0 ||
    value.forcedCleanup !== false ||
    !Array.isArray(value.activeProcessesAfterTeardown) ||
    value.activeProcessesAfterTeardown.length !== 0 ||
    residueBefore === null ||
    residueBefore.length !== 0 ||
    residueAfter === null ||
    JSON.stringify(residueBefore) !== JSON.stringify(residueAfter) ||
    !Array.isArray(value.newResidueAfterCancel) ||
    JSON.stringify([...value.newResidueAfterCancel].sort()) !==
      JSON.stringify(computedNewResidue) ||
    computedNewResidue.length !== 0 ||
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
  const parsedPackage = parseTqpkg2(installerBytes, architecture);
  const installerSha256 = createHash('sha256').update(installerBytes).digest('hex');
  const provenanceDocumentSha256 = createHash('sha256').update(provenanceSource).digest('hex');
  const finalEntries = provenance.entries.filter(({ role }) => role === 'final-artifact');
  const packageEntryHash = (suffix) =>
    provenance.entries.find(
      (entry) =>
        entry.role === 'package-file' &&
        entry.kind === 'file' &&
        entry.path.replaceAll('\\', '/').endsWith(`/${suffix}`),
    )?.sha256;
  const manifestFileHash = (path) =>
    parsedPackage.manifest.files.find((entry) => entry.path === path)?.sha256;
  if (
    !['x64', 'arm64'].includes(architecture) ||
    provenance.package.platform !== 'win' ||
    provenance.package.arch !== architecture ||
    parsedPackage.manifest.architecture !== architecture ||
    parsedPackage.manifest.packageMode !== 'fresh' ||
    parsedPackage.manifest.predecessor !== null ||
    parsedPackage.manifest.faultPhase !== null ||
    parsedPackage.manifest.version !== provenance.package.version ||
    parsedPackage.manifest.sourceCommit !== provenance.sourceCommit ||
    parsedPackage.manifest.sourceTree !== provenance.sourceTree ||
    parsedPackage.manifest.target.gatewaySha256 !==
      manifestFileHash('resources/helper/talking-quill-helper.exe') ||
    parsedPackage.manifest.target.ownerSha256 !==
      manifestFileHash('resources/helper/talking-quill-keyboard-owner.exe') ||
    parsedPackage.manifest.target.gatewaySha256 !==
      packageEntryHash('resources/helper/talking-quill-helper.exe') ||
    parsedPackage.manifest.target.ownerSha256 !==
      packageEntryHash('resources/helper/talking-quill-keyboard-owner.exe') ||
    manifestFileHash('resources/keyboard-owner-release-v1.json') !==
      packageEntryHash('resources/keyboard-owner-release-v1.json') ||
    !HEX_64.test(parsedPackage.manifest.target.releaseBuildDigest) ||
    !HEX_40.test(provenance.sourceCommit) ||
    !HEX_40.test(provenance.sourceTree) ||
    !HEX_64.test(provenance.sourceTreeSha256) ||
    !finalEntries.some(
      (entry) =>
        basename(entry.path) === basename(installerPath) && entry.sha256 === installerSha256,
    )
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
