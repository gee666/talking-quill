import { spawn } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { createReadStream } from 'node:fs';
import { cp, lstat, mkdir, readFile, readdir, realpath, readlink } from 'node:fs/promises';
import { dirname, isAbsolute, relative, resolve } from 'node:path';

const inspectedArtifacts = new WeakSet();
const launchableSnapshots = new WeakSet();

/** Resolves and hashes the complete artifact tree that a platform suite will launch. */
export async function inspectExactArtifact(options) {
  const root = await realpath(resolve(options.root));
  const requested = isAbsolute(options.executable)
    ? resolve(options.executable)
    : resolve(root, options.executable);
  const executable = await realpath(requested);
  const relativePath = relative(root, executable);
  if (relativePath === '' || relativePath.startsWith('..') || isAbsolute(relativePath)) {
    throw new Error('Exact-artifact executable must be contained by its declared artifact root');
  }
  const metadata = await lstat(requested);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error('Exact-artifact executable must be a regular non-symlink file');
  }
  const [sha256, treeSha256] = await Promise.all([hashFile(executable), hashTree(root)]);
  if (options.expectedSha256 !== undefined && sha256 !== options.expectedSha256.toLowerCase()) {
    throw new Error(
      `Exact-artifact executable SHA-256 mismatch: expected ${options.expectedSha256}`,
    );
  }
  if (
    options.expectedTreeSha256 !== undefined &&
    treeSha256 !== options.expectedTreeSha256.toLowerCase()
  ) {
    throw new Error(`Exact-artifact tree SHA-256 mismatch: expected ${options.expectedTreeSha256}`);
  }
  const artifact = Object.freeze({ root, executable, sha256, treeSha256 });
  inspectedArtifacts.add(artifact);
  return artifact;
}

/** Copies verified bytes into a private, single-use launch tree before process creation. */
export async function snapshotExactArtifact(artifact, snapshotParent) {
  await verifyExactArtifact(artifact);
  const parent = resolve(snapshotParent);
  await mkdir(parent, { recursive: true });
  const snapshotRoot = resolve(parent, `${artifact.treeSha256}-${randomUUID()}`);
  await cp(artifact.root, snapshotRoot, {
    recursive: true,
    force: false,
    errorOnExist: true,
    verbatimSymlinks: true,
  });
  const snapshot = await inspectExactArtifact({
    root: snapshotRoot,
    executable: relative(artifact.root, artifact.executable),
    expectedSha256: artifact.sha256,
    expectedTreeSha256: artifact.treeSha256,
  });
  launchableSnapshots.add(snapshot);
  return snapshot;
}

/** Re-hashes all bytes to detect replacement after inspection or execution. */
export async function verifyExactArtifact(artifact) {
  if (!inspectedArtifacts.has(artifact)) throw new Error('Exact artifact was not inspected here');
  const verified = await inspectExactArtifact({
    root: artifact.root,
    executable: artifact.executable,
    expectedSha256: artifact.sha256,
    expectedTreeSha256: artifact.treeSha256,
  });
  return verified;
}

/** Revalidates immediately before spawning and rejects caller-constructed artifact records. */
export async function launchExactArtifact(artifact, args, options = {}) {
  if (!launchableSnapshots.has(artifact)) {
    throw new Error('Exact artifact must be an isolated launch snapshot');
  }
  await verifyExactArtifact(artifact);
  return spawn(artifact.executable, args, {
    ...options,
    shell: false,
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
}

export function hashFile(path) {
  return new Promise((resolveHash, reject) => {
    const hash = createHash('sha256');
    const input = createReadStream(path);
    input.once('error', reject);
    input.on('data', (chunk) => hash.update(chunk));
    input.once('end', () => resolveHash(hash.digest('hex')));
  });
}

async function hashTree(root) {
  const records = [];
  await collectTree(root, '', records);
  const hash = createHash('sha256');
  for (const record of records) hash.update(record);
  return hash.digest('hex');
}

async function collectTree(root, relativeRoot, records) {
  const directory = resolve(root, relativeRoot);
  const entries = await readdir(directory, { withFileTypes: true });
  entries.sort((left, right) => left.name.localeCompare(right.name, 'en'));
  for (const entry of entries) {
    const relativePath = relativeRoot === '' ? entry.name : `${relativeRoot}/${entry.name}`;
    const path = resolve(root, relativePath);
    const metadata = await lstat(path);
    const mode = (metadata.mode & 0o777).toString(8);
    if (entry.isDirectory()) {
      records.push(Buffer.from(`d\0${relativePath}\0${mode}\0`, 'utf8'));
      await collectTree(root, relativePath, records);
    } else if (entry.isFile()) {
      const bytes = await readFile(path);
      const digest = createHash('sha256').update(bytes).digest('hex');
      records.push(Buffer.from(`f\0${relativePath}\0${mode}\0${digest}\0`, 'utf8'));
    } else if (entry.isSymbolicLink()) {
      const target = await readlink(path);
      const resolvedTarget = resolve(dirname(path), target);
      const targetRelative = relative(root, resolvedTarget);
      if (targetRelative === '' || targetRelative.startsWith('..') || isAbsolute(targetRelative)) {
        throw new Error(`Exact-artifact symlink escapes its root: ${relativePath}`);
      }
      records.push(Buffer.from(`l\0${relativePath}\0${target}\0`, 'utf8'));
    } else {
      throw new Error(`Exact-artifact tree contains an unsupported entry: ${relativePath}`);
    }
  }
}
