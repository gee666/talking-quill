import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

function versionParts(version) {
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/u.exec(version);
  if (match === null) throw new Error(`Windows recovery release version is invalid: ${version}`);
  return match.slice(1).map(Number);
}

function compareVersions(left, right) {
  for (let index = 0; index < 3; index += 1) {
    if (left[index] !== right[index]) return left[index] - right[index];
  }
  return 0;
}

export function verifyWindowsRecoveryLineage(lineage) {
  if (
    lineage === null ||
    typeof lineage !== 'object' ||
    lineage.schemaVersion !== 2 ||
    lineage.currentRelaunchRecordSchema !== 3 ||
    lineage.trustRootVersion !== '0.0.69' ||
    !Array.isArray(lineage.unsupportedUnpublishedRelaunchRecordSchemas) ||
    !Array.isArray(lineage.localMigrations) ||
    !Array.isArray(lineage.publishedArtifacts)
  ) {
    throw new Error('Windows recovery lineage is invalid');
  }
  const unsupported = new Set(lineage.unsupportedUnpublishedRelaunchRecordSchemas);
  if (unsupported.size !== 2 || !unsupported.has(1) || !unsupported.has(2)) {
    throw new Error('Windows recovery unpublished schema set is invalid');
  }
  if (
    lineage.localMigrations.length !== 1 ||
    JSON.stringify(lineage.localMigrations[0]) !==
      JSON.stringify({
        sourceVersion: '0.0.67',
        provenance: 'local-non-public',
        mode: 'local-uninstall-preserve-fresh',
        targetVersion: '0.0.69',
      })
  ) {
    throw new Error('Windows local migration policy is invalid');
  }
  let previous = [-1, -1, -1];
  const versions = new Set();
  for (const artifact of lineage.publishedArtifacts) {
    if (
      artifact === null ||
      typeof artifact !== 'object' ||
      typeof artifact.version !== 'string' ||
      !Object.hasOwn(artifact, 'predecessorVersion') ||
      !Object.hasOwn(artifact, 'relaunchRecordSchema')
    ) {
      throw new Error('Published Windows recovery artifact is invalid');
    }
    if (artifact.version === '0.0.67') {
      throw new Error('Local 0.0.67 must never be claimed as a public artifact');
    }
    const version = versionParts(artifact.version);
    if (versions.has(artifact.version) || compareVersions(version, previous) <= 0) {
      throw new Error('Published Windows recovery artifacts are not strictly ordered');
    }
    versions.add(artifact.version);
    previous = version;
    if (
      artifact.relaunchRecordSchema !== lineage.currentRelaunchRecordSchema ||
      unsupported.has(artifact.relaunchRecordSchema)
    ) {
      throw new Error('Published artifact uses an invalid Windows recovery schema');
    }
  }
  const root = lineage.publishedArtifacts[0];
  if (
    root?.version !== lineage.trustRootVersion ||
    root.predecessorVersion !== null ||
    root.relaunchRecordSchema !== 3
  ) {
    throw new Error('Windows 0.0.69 must be the fresh public trust-lineage root');
  }
  for (let index = 1; index < lineage.publishedArtifacts.length; index += 1) {
    const artifact = lineage.publishedArtifacts[index];
    const predecessor = lineage.publishedArtifacts[index - 1];
    if (artifact.predecessorVersion !== predecessor.version) {
      throw new Error('Published Windows update predecessor is not the prior public release');
    }
  }
  return {
    trustRootVersion: root.version,
    localMigrationSourceVersion: lineage.localMigrations[0].sourceVersion,
    schemaVersion: lineage.currentRelaunchRecordSchema,
    publishedArtifacts: lineage.publishedArtifacts.length,
  };
}

export async function verifyWindowsRecoveryLineageFile(configPath) {
  const config = JSON.parse(await readFile(configPath, 'utf8'));
  return verifyWindowsRecoveryLineage(config.windowsRecoveryLineage);
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  await verifyWindowsRecoveryLineageFile(resolve(process.argv[2] ?? 'release.config.json'));
  console.log('Windows recovery release lineage is valid.');
}
