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
    lineage.schemaVersion !== 1 ||
    lineage.currentRelaunchRecordSchema !== 3 ||
    !Array.isArray(lineage.unsupportedUnpublishedRelaunchRecordSchemas) ||
    !Array.isArray(lineage.publishedArtifacts)
  ) {
    throw new Error('Windows recovery lineage is invalid');
  }
  const unsupported = new Set(lineage.unsupportedUnpublishedRelaunchRecordSchemas);
  if (unsupported.size !== 2 || !unsupported.has(1) || !unsupported.has(2)) {
    throw new Error('Windows recovery unpublished schema set is invalid');
  }
  if (unsupported.has(lineage.currentRelaunchRecordSchema)) {
    throw new Error('Current Windows recovery schema is marked unpublished');
  }
  let previous = [-1, -1, -1];
  const versions = new Set();
  for (const artifact of lineage.publishedArtifacts) {
    if (
      artifact === null ||
      typeof artifact !== 'object' ||
      typeof artifact.version !== 'string' ||
      !('relaunchRecordSchema' in artifact)
    ) {
      throw new Error('Published Windows recovery artifact is invalid');
    }
    const version = versionParts(artifact.version);
    if (versions.has(artifact.version) || compareVersions(version, previous) <= 0) {
      throw new Error('Published Windows recovery artifacts are not strictly ordered');
    }
    versions.add(artifact.version);
    previous = version;
    const schema = artifact.relaunchRecordSchema;
    if (schema !== null && (!Number.isSafeInteger(schema) || schema <= 0)) {
      throw new Error('Published Windows recovery artifact schema is invalid');
    }
    if (schema !== null && unsupported.has(schema)) {
      throw new Error('Published artifact uses an unpublished Windows recovery schema');
    }
  }
  const baseline = lineage.publishedArtifacts[0];
  const current = lineage.publishedArtifacts[1];
  if (baseline?.version !== '0.0.67' || baseline.relaunchRecordSchema !== null) {
    throw new Error('Windows recovery no-record baseline must be public 0.0.67');
  }
  if (
    current?.version !== '0.0.69' ||
    current.relaunchRecordSchema !== lineage.currentRelaunchRecordSchema ||
    lineage.publishedArtifacts
      .slice(1)
      .some((artifact) => artifact.relaunchRecordSchema !== lineage.currentRelaunchRecordSchema)
  ) {
    throw new Error('Windows recovery schema 3 lineage must begin at 0.0.69');
  }
  return {
    baselineVersion: baseline.version,
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
