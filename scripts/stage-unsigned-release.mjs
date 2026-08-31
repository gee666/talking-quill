import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, rename, rm, stat, writeFile } from 'node:fs/promises';
import { basename, relative, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { dump, load } from 'js-yaml';
import {
  authorizeWindowsUpdaterReleaseBinding,
  createUpdaterReleaseBinding,
  RELEASE_PACKAGE_METADATA_NAME,
  validatePackageReleaseMetadata,
} from './release-package-metadata.mjs';
import {
  validateArtifactProvenanceManifest,
  verifyArtifactProvenanceManifest,
} from './artifact-provenance.mjs';

async function main() {
  const [platform, arch] = process.argv.slice(2).filter((value) => value !== '--');
  if (!['win', 'mac'].includes(platform) || !['x64', 'arm64'].includes(arch)) {
    throw new Error('Usage: stage-unsigned-release <win|mac> <x64|arm64>');
  }
  const root = resolve(import.meta.dirname, '..');
  const release = resolve(root, 'release');
  const output = resolve(root, 'tmp', 'release-upload');
  const pendingOutput = resolve(root, 'tmp', `release-upload.pending-${String(process.pid)}`);
  const manifest = JSON.parse(await readFile(resolve(root, 'app/package.json'), 'utf8'));
  const version = manifest.version;
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+$/u.test(version)) {
    throw new Error('Application version must be strict three-part semver.');
  }
  const stem = `Talking-Quill-${version}-${platform}-${arch}`;
  const finalNames =
    platform === 'win'
      ? [`${stem}-setup.exe`, `${stem}-update.exe`]
      : [`${stem}.dmg`, `${stem}.zip`];
  const updateName = platform === 'win' ? `${stem}-update.exe` : `${stem}.zip`;
  const blockmapName = `${updateName}.blockmap`;
  const rawMetadataName = platform === 'win' ? 'latest.yml' : 'latest-mac.yml';
  const channelMetadataName = platform === 'win' ? `latest-${arch}.yml` : `latest-${arch}-mac.yml`;

  const packageRoot = packageRootForTarget(release, platform, arch);
  const packageMetadataPath =
    platform === 'win'
      ? resolve(packageRoot, 'resources', RELEASE_PACKAGE_METADATA_NAME)
      : resolve(
          packageRoot,
          'Talking Quill.app',
          'Contents',
          'Resources',
          RELEASE_PACKAGE_METADATA_NAME,
        );
  const packageMetadata = validatePackageReleaseMetadata(
    JSON.parse(await readFile(packageMetadataPath, 'utf8')),
  );
  const provenance = await verifyArtifactProvenanceManifest();
  const provenanceRoot = relative(root, packageRoot).replaceAll('\\', '/');
  if (
    provenance.package.version !== version ||
    provenance.package.platform !== platform ||
    provenance.package.arch !== arch ||
    provenance.package.root !== provenanceRoot ||
    packageMetadata.sourceCommit !== provenance.sourceCommit ||
    packageMetadata.sourceTree !== provenance.sourceTree
  ) {
    throw new Error('Artifact provenance does not match the package selected for staging.');
  }
  if (
    packageMetadata.version !== version ||
    packageMetadata.platform !== platform ||
    packageMetadata.architecture !== arch ||
    (platform === 'mac' &&
      (packageMetadata.predecessor === null ||
        packageMetadata.outerIdentity?.mode !== 'certificate'))
  ) {
    throw new Error('Serialized package metadata does not match the updater target.');
  }
  const updateEvidence = await fileEvidence(resolve(release, updateName));
  const unsignedBinding = createUpdaterReleaseBinding(packageMetadata, updateEvidence.sha256);
  const releaseBinding =
    platform === 'win' ? authorizeWindowsUpdaterReleaseBinding(unsignedBinding) : unsignedBinding;
  const metadata = load(await readFile(resolve(release, rawMetadataName), 'utf8'));
  const channelMetadata = await canonicalizeUpdateMetadata(metadata, {
    expectedVersion: version,
    allowedFiles: finalNames,
    expectedUpdateFile: updateName,
    evidence: (name) => fileEvidence(resolve(release, name)),
    releaseBinding,
  });
  await requireFile(resolve(release, blockmapName));
  for (const name of finalNames) await requireFile(resolve(release, name));

  await rm(pendingOutput, { recursive: true, force: true });
  await mkdir(pendingOutput, { recursive: true });
  for (const name of [...finalNames, blockmapName]) {
    await copyFile(resolve(release, name), resolve(pendingOutput, name));
  }
  await writeFile(
    resolve(pendingOutput, channelMetadataName),
    dump(channelMetadata, { lineWidth: 120 }),
    'utf8',
  );
  await writeFile(
    resolve(pendingOutput, `release-identity-${platform}-${arch}.json`),
    `${JSON.stringify(releaseBinding, null, 2)}\n`,
    'utf8',
  );
  const freshProvenance =
    platform === 'win'
      ? JSON.parse(
          await readFile(
            resolve(root, process.env.TALKING_QUILL_FRESH_PROVENANCE_PATH ?? ''),
            'utf8',
          ),
        )
      : provenance;
  validateArtifactProvenanceManifest(freshProvenance);
  const provenanceDocuments =
    platform === 'win'
      ? [
          [`${platform}-${arch}-setup`, freshProvenance],
          [`${platform}-${arch}-update`, provenance],
        ]
      : [[`${platform}-${arch}`, provenance]];
  for (const [name, document] of provenanceDocuments) {
    await writeFile(
      resolve(pendingOutput, `provenance-${name}.json`),
      `${JSON.stringify(document, null, 2)}\n`,
      'utf8',
    );
  }
  if (platform === 'win') {
    await copyFile(
      resolve(root, 'app/assets/THIRD_PARTY_NOTICES.txt'),
      resolve(pendingOutput, 'THIRD_PARTY_NOTICES.txt'),
    );
  }
  const finalEntries = provenanceDocuments.flatMap(([, document]) =>
    document.entries.filter(({ role }) => role === 'final-artifact'),
  );
  if (
    JSON.stringify(finalEntries.map(({ path }) => basename(path)).sort()) !==
    JSON.stringify([...finalNames].sort())
  ) {
    throw new Error('Mode-specific provenance does not match staged release files.');
  }
  for (const entry of finalEntries) {
    const name = basename(entry.path);
    if ((await fileEvidence(resolve(pendingOutput, name))).sha256 !== entry.sha256) {
      throw new Error(`Staged artifact differs from provenance: ${name}`);
    }
  }
  await rm(output, { recursive: true, force: true });
  await rename(pendingOutput, output);
  console.log(
    `Staged unsigned ${platform}/${arch} release payload, blockmap, updater channel, notices, and provenance.`,
  );
}

export function packageRootForTarget(release, platform, architecture) {
  if (platform === 'win') {
    return resolve(release, architecture === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked');
  }
  return resolve(release, architecture === 'x64' ? 'mac' : 'mac-arm64');
}

export async function canonicalizeUpdateMetadata(
  value,
  { expectedVersion, allowedFiles, expectedUpdateFile, evidence, releaseBinding },
) {
  if (value === null || typeof value !== 'object' || Array.isArray(value)) {
    throw new Error('Generated updater metadata is not an object.');
  }
  const files = value.files;
  if (value.version !== expectedVersion || !Array.isArray(files) || files.length === 0) {
    throw new Error('Generated updater metadata identity is invalid.');
  }
  const allowed = new Set(allowedFiles);
  const seen = new Set();
  let updateEntry = null;
  for (const file of files) {
    if (file === null || typeof file !== 'object' || typeof file.url !== 'string') {
      throw new Error('Generated updater file metadata is invalid.');
    }
    const name = file.url;
    if (basename(name) !== name || !allowed.has(name) || seen.has(name)) {
      throw new Error(`Generated updater metadata contains an unexpected path: ${name}`);
    }
    seen.add(name);
    const payloadEvidence = await evidence(name);
    if (file.sha512 !== payloadEvidence.sha512 || file.size !== payloadEvidence.size) {
      throw new Error(`Generated updater metadata does not match payload bytes: ${name}`);
    }
    if (name === expectedUpdateFile) {
      updateEntry = {
        url: name,
        sha512: payloadEvidence.sha512,
        size: payloadEvidence.size,
        ...(Number.isSafeInteger(file.blockMapSize) && file.blockMapSize >= 0
          ? { blockMapSize: file.blockMapSize }
          : {}),
      };
    }
  }
  const updateEvidence = await evidence(expectedUpdateFile);
  if (
    updateEntry === null ||
    value.path !== expectedUpdateFile ||
    value.sha512 !== updateEvidence.sha512
  ) {
    throw new Error('Generated updater metadata does not select the expected update payload.');
  }
  if (
    releaseBinding !== undefined &&
    (releaseBinding?.schemaVersion !== 1 ||
      releaseBinding.version !== expectedVersion ||
      releaseBinding.packageSha256 !== updateEvidence.sha256 ||
      releaseBinding.transactionBinding !== 'source-target-package-sha256-v1')
  ) {
    throw new Error('Updater transaction binding does not match the exact payload bytes.');
  }
  return {
    version: expectedVersion,
    files: [updateEntry],
    path: expectedUpdateFile,
    sha512: updateEvidence.sha512,
    ...(typeof value.releaseDate === 'string' ? { releaseDate: value.releaseDate } : {}),
    ...(releaseBinding === undefined ? {} : { talkingQuillRelease: releaseBinding }),
  };
}

async function fileEvidence(path) {
  const bytes = await readFile(path);
  return {
    size: bytes.length,
    sha512: createHash('sha512').update(bytes).digest('base64'),
    sha256: createHash('sha256').update(bytes).digest('hex'),
  };
}

async function requireFile(path) {
  const metadata = await stat(path);
  if (!metadata.isFile() || metadata.size === 0)
    throw new Error(`Required release file is empty: ${path}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
