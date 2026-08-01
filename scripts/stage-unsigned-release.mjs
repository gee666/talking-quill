import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, rm, stat, writeFile } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { dump, load } from 'js-yaml';

async function main() {
  const [platform, arch] = process.argv.slice(2).filter((value) => value !== '--');
  if (!['win', 'mac'].includes(platform) || !['x64', 'arm64'].includes(arch)) {
    throw new Error('Usage: stage-unsigned-release <win|mac> <x64|arm64>');
  }
  const root = resolve(import.meta.dirname, '..');
  const release = resolve(root, 'release');
  const output = resolve(root, 'tmp', 'release-upload');
  const manifest = JSON.parse(await readFile(resolve(root, 'app/package.json'), 'utf8'));
  const version = manifest.version;
  if (typeof version !== 'string' || !/^\d+\.\d+\.\d+$/u.test(version)) {
    throw new Error('Application version must be strict three-part semver.');
  }
  const stem = `Talking-Quill-${version}-${platform}-${arch}`;
  const finalNames = platform === 'win' ? [`${stem}.exe`] : [`${stem}.dmg`, `${stem}.zip`];
  const updateName = platform === 'win' ? `${stem}.exe` : `${stem}.zip`;
  const blockmapName = `${updateName}.blockmap`;
  const rawMetadataName = platform === 'win' ? 'latest.yml' : 'latest-mac.yml';
  const channelMetadataName = platform === 'win' ? `latest-${arch}.yml` : `latest-${arch}-mac.yml`;

  const metadata = load(await readFile(resolve(release, rawMetadataName), 'utf8'));
  const channelMetadata = await canonicalizeUpdateMetadata(metadata, {
    expectedVersion: version,
    allowedFiles: finalNames,
    expectedUpdateFile: updateName,
    evidence: (name) => fileEvidence(resolve(release, name)),
  });
  await requireFile(resolve(release, blockmapName));
  await requireFile(resolve(root, 'artifact-provenance.json'));
  for (const name of finalNames) await requireFile(resolve(release, name));

  await rm(output, { recursive: true, force: true });
  await mkdir(output, { recursive: true });
  for (const name of [...finalNames, blockmapName]) {
    await copyFile(resolve(release, name), resolve(output, name));
  }
  await writeFile(
    resolve(output, channelMetadataName),
    dump(channelMetadata, { lineWidth: 120 }),
    'utf8',
  );
  await copyFile(
    resolve(root, 'artifact-provenance.json'),
    resolve(output, `provenance-${platform}-${arch}.json`),
  );
  console.log(
    `Staged unsigned ${platform}/${arch} release payload, blockmap, updater channel, and provenance.`,
  );
}

export async function canonicalizeUpdateMetadata(
  value,
  { expectedVersion, allowedFiles, expectedUpdateFile, evidence },
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
  return {
    version: expectedVersion,
    files: [updateEntry],
    path: expectedUpdateFile,
    sha512: updateEvidence.sha512,
    ...(typeof value.releaseDate === 'string' ? { releaseDate: value.releaseDate } : {}),
  };
}

async function fileEvidence(path) {
  const bytes = await readFile(path);
  return {
    size: bytes.length,
    sha512: createHash('sha512').update(bytes).digest('base64'),
  };
}

async function requireFile(path) {
  const metadata = await stat(path);
  if (!metadata.isFile() || metadata.size === 0)
    throw new Error(`Required release file is empty: ${path}`);
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) await main();
