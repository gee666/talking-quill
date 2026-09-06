import { createHash } from 'node:crypto';
import { readFile, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { parseTqpkg2 } from './tqpkg2.mjs';
import { validateArtifactProvenanceManifest } from './artifact-provenance.mjs';

const sha256 = (bytes) => createHash('sha256').update(bytes).digest('hex');
export async function hostedInstallerBinding(installerPath, provenancePath, architecture) {
  const bytes = await readFile(installerPath);
  const provenanceBytes = await readFile(provenancePath);
  const provenance = JSON.parse(provenanceBytes.toString('utf8'));
  validateArtifactProvenanceManifest(provenance);
  const { manifest } = parseTqpkg2(bytes, architecture);
  const final = provenance.entries.filter((entry) => entry.role === 'final-artifact');
  if (
    !['x64', 'arm64'].includes(architecture) ||
    manifest.architecture !== architecture ||
    manifest.packageMode !== 'fresh' ||
    manifest.predecessor !== null ||
    manifest.faultPhase !== null ||
    provenance.package.arch !== architecture ||
    provenance.package.platform !== 'win' ||
    provenance.package.version !== manifest.version ||
    provenance.sourceCommit !== manifest.sourceCommit ||
    provenance.sourceTree !== manifest.sourceTree ||
    final.length !== 1 ||
    basename(final[0].path) !== basename(installerPath) ||
    final[0].sha256 !== sha256(bytes)
  ) {
    throw new Error(
      'Hosted lifecycle input is not the exact provenance-bound fresh production installer',
    );
  }
  for (const [file, targetHash] of [
    ['resources/helper/talking-quill-helper.exe', manifest.target.gatewaySha256],
    ['resources/helper/talking-quill-keyboard-owner.exe', manifest.target.ownerSha256],
  ]) {
    if (
      manifest.files.find((entry) => entry.path === file)?.sha256 !== targetHash ||
      !provenance.entries.some(
        (entry) =>
          entry.role === 'package-file' &&
          entry.path.replaceAll('\\', '/').endsWith(`/${file}`) &&
          entry.sha256 === targetHash,
      )
    )
      throw new Error('Hosted lifecycle role hashes do not match package provenance');
  }
  return {
    architecture,
    sourceCommit: provenance.sourceCommit,
    sourceTree: provenance.sourceTree,
    sourceTreeSha256: provenance.sourceTreeSha256,
    version: manifest.version,
    installer: basename(installerPath),
    installerSha256: sha256(bytes),
    bytes: bytes.length,
    provenanceDocumentSha256: sha256(provenanceBytes),
    files: manifest.files,
  };
}

export function validateHostedLifecycleEvidence(value, binding) {
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
    value.schemaVersion !== 1 ||
    value.kind !== 'github-hosted-elevated-install-lifecycle' ||
    value.result !== 'passed' ||
    fields.some((key) => value[key] !== binding[key]) ||
    !/^[1-9][0-9]*$/u.test(value.workflowRunId) ||
    value.host !== 'github-hosted' ||
    value.elevated !== true ||
    value.uac !== 'not-exercised' ||
    value.machineStateBefore !== 'absent' ||
    value.machineStateAfter !== 'absent' ||
    value.installExitCode !== 0 ||
    value.uninstallExitCode !== 0 ||
    value.installedFileCount !== binding.files.length ||
    value.installedFilesVerified !== true ||
    value.lifecycle?.result !== 'passed' ||
    value.lifecycle.architecture !== binding.architecture ||
    value.lifecycle.mode !== 'installed' ||
    value.lifecycle.first?.result !== 'passed' ||
    value.lifecycle.successor?.result !== 'passed' ||
    value.lifecycle.crash?.ownerAuthenticated !== true ||
    value.lifecycle.ownershipCoverage?.authoritative !== false
  ) {
    throw new Error(
      'Hosted install/lifecycle evidence does not prove the exact observed install and removal',
    );
  }
  return value;
}

export async function verifyHostedLifecycleEvidence({
  evidencePath,
  installerPath,
  provenancePath,
  architecture,
}) {
  const binding = await hostedInstallerBinding(installerPath, provenancePath, architecture);
  return validateHostedLifecycleEvidence(JSON.parse(await readFile(evidencePath, 'utf8')), binding);
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [mode, installer, provenance, architecture, path] = process.argv.slice(2);
  if (!['--prepare', '--verify'].includes(mode) || !path)
    throw new Error('Expected --prepare|--verify INSTALLER PROVENANCE ARCH OUTPUT_OR_EVIDENCE');
  if (mode === '--prepare')
    await writeFile(
      path,
      JSON.stringify(await hostedInstallerBinding(installer, provenance, architecture)),
    );
  else
    await verifyHostedLifecycleEvidence({
      evidencePath: path,
      installerPath: installer,
      provenancePath: provenance,
      architecture,
    });
}
