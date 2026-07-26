import { createHash } from 'node:crypto';
import { constants } from 'node:fs';
import {
  lstat,
  mkdir,
  open,
  readFile,
  readdir,
  readlink,
  realpath,
  rename,
  writeFile,
} from 'node:fs/promises';
import { spawnSync } from 'node:child_process';
import { dirname, isAbsolute, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
export const artifactProvenanceManifestPath = resolve(repositoryRoot, 'artifact-provenance.json');
const temporaryManifestPath = resolve(repositoryRoot, 'tmp', 'artifact-provenance.json.pending');

export async function writeArtifactProvenanceManifest(options) {
  const identity = validateIdentity(options);
  const packageRoot = await containedRealPath(options.packageRoot);
  const packageRootPath = repositoryRelative(packageRoot);
  const artifacts = await Promise.all(
    [...options.artifacts]
      .map((path) => resolve(path))
      .sort((left, right) => left.localeCompare(right))
      .map(async (path) => ({ ...(await fileEntry(path)), role: 'final-artifact' })),
  );
  const packageFiles = (await inventoryTree(packageRoot)).map((entry) => ({
    ...entry,
    role: 'package-file',
  }));
  const manifest = {
    schemaVersion: 1,
    sourceCommit: currentCommit(),
    sourceTreeSha256: await currentSourceTreeHash(),
    package: { ...identity, root: packageRootPath },
    entries: [...packageFiles, ...artifacts].sort(compareEntries),
  };
  validateArtifactProvenanceManifest(manifest);
  await mkdir(dirname(temporaryManifestPath), { recursive: true });
  await writeFile(temporaryManifestPath, `${JSON.stringify(manifest, null, 2)}\n`, {
    encoding: 'utf8',
    mode: 0o600,
  });
  await rename(temporaryManifestPath, artifactProvenanceManifestPath);
  return manifest;
}

export async function verifyArtifactProvenanceManifest() {
  const source = await readFile(artifactProvenanceManifestPath, 'utf8').catch(() => {
    throw new Error(`Artifact provenance manifest is missing: ${artifactProvenanceManifestPath}`);
  });
  let manifest;
  try {
    manifest = JSON.parse(source);
  } catch {
    throw new Error('Artifact provenance manifest is not valid JSON');
  }
  validateArtifactProvenanceManifest(manifest);
  if (manifest.sourceCommit !== currentCommit()) {
    throw new Error('Artifact provenance manifest is stale for the current source commit');
  }
  if (manifest.sourceTreeSha256 !== (await currentSourceTreeHash())) {
    throw new Error('Artifact provenance manifest is stale for the current source tree');
  }
  const appManifest = JSON.parse(
    await readFile(resolve(repositoryRoot, 'app/package.json'), 'utf8'),
  );
  if (manifest.package.version !== appManifest.version) {
    throw new Error('Artifact provenance manifest is stale for the application version');
  }
  const expectedPlatform = process.env.TALKING_QUILL_PACKAGE_TARGET;
  const expectedArchitecture = process.env.TALKING_QUILL_PACKAGE_ARCH;
  const expectedRoot = process.env.TALKING_QUILL_PACKAGE_ROOT;
  if (
    (expectedPlatform !== undefined && manifest.package.platform !== expectedPlatform) ||
    (expectedArchitecture !== undefined && manifest.package.arch !== expectedArchitecture) ||
    (expectedRoot !== undefined &&
      manifest.package.root !== repositoryRelative(resolve(repositoryRoot, expectedRoot)))
  ) {
    throw new Error('Artifact provenance manifest does not match the requested package identity');
  }

  const packageRoot = resolveManifestPath(manifest.package.root);
  await assertCanonicalRealPath(packageRoot, 'package root');
  const actualEntries = [
    ...(await inventoryTree(packageRoot)).map((entry) => ({ ...entry, role: 'package-file' })),
    ...(await Promise.all(
      manifest.entries
        .filter((entry) => entry.role === 'final-artifact')
        .map(async (entry) => ({
          ...(await fileEntry(resolveManifestPath(entry.path))),
          role: entry.role,
        })),
    )),
  ].sort(compareEntries);
  if (JSON.stringify(actualEntries) !== JSON.stringify(manifest.entries)) {
    throw new Error(
      'Artifact provenance manifest does not match current package bytes or inventory',
    );
  }
  return manifest;
}

export function artifactUploadPaths(manifest) {
  validateArtifactProvenanceManifest(manifest);
  return [
    `${manifest.package.root}/**`,
    ...manifest.entries
      .filter((entry) => entry.role === 'final-artifact')
      .map((entry) => entry.path),
    repositoryRelative(artifactProvenanceManifestPath),
  ];
}

async function inventoryTree(root) {
  const entries = [];
  async function walk(directory) {
    const children = await readdir(directory, { withFileTypes: true });
    children.sort((left, right) => left.name.localeCompare(right.name));
    for (const child of children) {
      const path = resolve(directory, child.name);
      const metadata = await lstat(path);
      if (metadata.isSymbolicLink()) {
        entries.push({
          path: repositoryRelative(path),
          kind: 'symlink',
          target: await readlink(path),
        });
      } else if (metadata.isDirectory()) {
        await walk(path);
      } else if (metadata.isFile()) {
        entries.push(await fileEntry(path));
      } else {
        throw new Error(`Unsupported package filesystem entry: ${repositoryRelative(path)}`);
      }
    }
  }
  await walk(root);
  return entries.sort(compareEntries);
}

async function fileEntry(path) {
  const canonical = resolve(path);
  await assertCanonicalRealPath(canonical, 'file');
  const before = await lstat(canonical);
  if (!before.isFile() || before.isSymbolicLink()) {
    throw new Error(`Provenance artifact is not a regular file: ${repositoryRelative(canonical)}`);
  }
  const handle = await open(canonical, constants.O_RDONLY | constants.O_NOFOLLOW);
  try {
    const opened = await handle.stat();
    if (
      !opened.isFile() ||
      opened.dev !== before.dev ||
      opened.ino !== before.ino ||
      opened.size !== before.size
    ) {
      throw new Error(
        `Provenance artifact changed before hashing: ${repositoryRelative(canonical)}`,
      );
    }
    const hash = createHash('sha256');
    const buffer = Buffer.alloc(1024 * 1024);
    let position = 0;
    while (position < opened.size) {
      const { bytesRead } = await handle.read(
        buffer,
        0,
        Math.min(buffer.length, opened.size - position),
        position,
      );
      if (bytesRead === 0) throw new Error('Unexpected end of provenance artifact');
      hash.update(buffer.subarray(0, bytesRead));
      position += bytesRead;
    }
    const [afterHandle, afterPath] = await Promise.all([handle.stat(), lstat(canonical)]);
    if (
      afterHandle.dev !== opened.dev ||
      afterHandle.ino !== opened.ino ||
      afterHandle.size !== opened.size ||
      afterPath.isSymbolicLink() ||
      afterPath.dev !== opened.dev ||
      afterPath.ino !== opened.ino ||
      afterPath.size !== opened.size
    ) {
      throw new Error(
        `Provenance artifact changed while hashing: ${repositoryRelative(canonical)}`,
      );
    }
    return {
      path: repositoryRelative(canonical),
      kind: 'file',
      size: opened.size,
      sha256: hash.digest('hex'),
    };
  } finally {
    await handle.close();
  }
}

async function containedRealPath(path) {
  const canonical = await realpath(resolve(path));
  repositoryRelative(canonical);
  return canonical;
}

async function assertCanonicalRealPath(path, label) {
  const canonical = resolve(path);
  const physical = await realpath(canonical);
  repositoryRelative(physical);
  const comparable = (value) => (process.platform === 'win32' ? value.toLowerCase() : value);
  if (comparable(physical) !== comparable(canonical)) {
    throw new Error(`Artifact provenance ${label} is not a canonical physical path: ${path}`);
  }
}

function repositoryRelative(path) {
  const name = relative(repositoryRoot, resolve(path));
  if (name.length === 0 || isAbsolute(name) || name === '..' || name.startsWith(`..${sep}`)) {
    throw new Error(`Artifact provenance path escapes the canonical repository root: ${path}`);
  }
  return name.replaceAll('\\', '/');
}

function resolveManifestPath(path) {
  if (typeof path !== 'string' || path.length === 0 || path.includes('\\')) {
    throw new Error('Artifact provenance manifest contains an invalid path');
  }
  const absolute = resolve(repositoryRoot, path);
  if (repositoryRelative(absolute) !== path) {
    throw new Error(`Artifact provenance manifest path is not canonical: ${path}`);
  }
  return absolute;
}

function validateIdentity(options) {
  if (
    options === null ||
    typeof options !== 'object' ||
    !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/u.test(options.version ?? '') ||
    !['win', 'mac'].includes(options.platform) ||
    !['x64', 'arm64'].includes(options.arch) ||
    !Array.isArray(options.artifacts)
  ) {
    throw new Error('Artifact provenance identity is invalid');
  }
  return { version: options.version, platform: options.platform, arch: options.arch };
}

export function validateArtifactProvenanceManifest(manifest) {
  if (
    manifest === null ||
    typeof manifest !== 'object' ||
    manifest.schemaVersion !== 1 ||
    typeof manifest.sourceCommit !== 'string' ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceCommit) ||
    typeof manifest.sourceTreeSha256 !== 'string' ||
    !/^[0-9a-f]{64}$/u.test(manifest.sourceTreeSha256) ||
    manifest.package === null ||
    typeof manifest.package !== 'object' ||
    !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/u.test(manifest.package.version ?? '') ||
    !['win', 'mac'].includes(manifest.package.platform) ||
    !['x64', 'arm64'].includes(manifest.package.arch) ||
    !Array.isArray(manifest.entries)
  ) {
    throw new Error('Artifact provenance manifest has an invalid schema');
  }
  resolveManifestPath(manifest.package.root);
  let previous = '';
  const seen = new Set();
  for (const entry of manifest.entries) {
    if (
      entry === null ||
      typeof entry !== 'object' ||
      !['package-file', 'final-artifact'].includes(entry.role) ||
      !['file', 'symlink'].includes(entry.kind)
    ) {
      throw new Error('Artifact provenance manifest has an invalid entry');
    }
    resolveManifestPath(entry.path);
    const key = `${entry.role}:${entry.path}`;
    if (seen.has(key) || (previous.length > 0 && previous.localeCompare(key) > 0)) {
      throw new Error('Artifact provenance manifest entries are duplicate or unsorted');
    }
    seen.add(key);
    previous = key;
    if (entry.kind === 'file') {
      if (
        !Number.isSafeInteger(entry.size) ||
        entry.size < 0 ||
        !/^[0-9a-f]{64}$/u.test(entry.sha256)
      ) {
        throw new Error('Artifact provenance manifest has invalid file evidence');
      }
      if ('target' in entry) throw new Error('Artifact provenance file entry has a link target');
    } else {
      if (
        entry.role !== 'package-file' ||
        typeof entry.target !== 'string' ||
        entry.target.length === 0
      ) {
        throw new Error('Artifact provenance manifest has invalid link evidence');
      }
      if ('size' in entry || 'sha256' in entry) {
        throw new Error('Artifact provenance link entry has file evidence');
      }
    }
  }
}

function compareEntries(left, right) {
  return `${left.role ?? ''}:${left.path}`.localeCompare(`${right.role ?? ''}:${right.path}`);
}

async function currentSourceTreeHash() {
  if (process.env.TALKING_QUILL_REQUIRE_CLEAN_SOURCE === '1') {
    const status = spawnSync('git', ['status', '--porcelain=v1', '--untracked-files=normal'], {
      cwd: repositoryRoot,
      encoding: 'utf8',
      timeout: 30_000,
    });
    if (status.status !== 0 || status.stdout.trim().length > 0) {
      throw new Error('Release provenance requires a clean source tree.');
    }
  }
  const options = {
    cwd: repositoryRoot,
    encoding: null,
    timeout: 30_000,
    maxBuffer: 64 * 1024 * 1024,
  };
  const tree = spawnSync('git', ['ls-tree', '-rz', '--full-tree', 'HEAD'], options);
  const diff = spawnSync('git', ['diff', '--binary', '--no-ext-diff', 'HEAD', '--', '.'], options);
  const untracked = spawnSync('git', ['ls-files', '--others', '--exclude-standard', '-z'], options);
  if (tree.status !== 0 || diff.status !== 0 || untracked.status !== 0) {
    throw new Error('Unable to bind artifact provenance to the current source tree');
  }
  const hash = createHash('sha256')
    .update('talking-quill-source-tree-v2\0')
    .update(tree.stdout)
    .update(diff.stdout);
  const paths = untracked.stdout.toString('utf8').split('\0').filter(Boolean).sort();
  for (const path of paths) {
    const absolute = resolve(repositoryRoot, path);
    const metadata = await lstat(absolute);
    hash.update(path).update('\0');
    if (metadata.isSymbolicLink()) hash.update(await readlink(absolute));
    else if (metadata.isFile()) hash.update(await readFile(absolute));
    else throw new Error(`Unsupported untracked source entry: ${path}`);
    hash.update('\0');
  }
  return hash.digest('hex');
}

function currentCommit() {
  const result = spawnSync('git', ['rev-parse', 'HEAD'], {
    cwd: repositoryRoot,
    encoding: 'utf8',
    timeout: 10_000,
  });
  const commit = result.stdout.trim();
  if (result.status !== 0 || !/^[0-9a-f]{40}$/u.test(commit)) {
    throw new Error('Unable to bind artifact provenance to the current source commit');
  }
  const expectedCommit = process.env.TALKING_QUILL_RELEASE_COMMIT;
  if (expectedCommit !== undefined && commit !== expectedCommit) {
    throw new Error('Artifact provenance checkout does not match the resolved release commit');
  }
  return commit;
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) {
  if (!process.argv.includes('--verify')) throw new Error('Expected --verify');
  const manifest = await verifyArtifactProvenanceManifest();
  console.log(
    `Artifact provenance verified (${manifest.entries.length} entries, ${manifest.package.platform}/${manifest.package.arch}).`,
  );
}
