import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(import.meta.dirname, '..');
const allowlistPath = resolve(root, 'scripts/reference-independence-allowlist.json');

export function pathContainsReferenceComponent(path) {
  return path
    .replaceAll('\\', '/')
    .split('/')
    .some((component) => component.toLowerCase() === 'reference');
}

export function compareBlobInventory(current, reference, allowlist) {
  const referenceByBlob = new Map();
  for (const entry of reference) {
    const sources = referenceByBlob.get(entry.blob) ?? [];
    sources.push(entry.path);
    referenceByBlob.set(entry.blob, sources);
  }
  const approved = new Map(
    allowlist.entries.map((entry) => [`${entry.path}\0${entry.blob}`, entry]),
  );
  const matched = new Set();
  const violations = [];
  for (const entry of current) {
    if (pathContainsReferenceComponent(entry.path)) violations.push(`forbidden-path:${entry.path}`);
    const sources = referenceByBlob.get(entry.blob);
    if (sources === undefined) continue;
    const key = `${entry.path}\0${entry.blob}`;
    const exception = approved.get(key);
    if (
      exception === undefined ||
      JSON.stringify([...exception.sourcePaths].sort()) !== JSON.stringify([...sources].sort())
    ) {
      violations.push(`copied-blob:${entry.path}:${entry.blob}`);
    } else {
      matched.add(key);
    }
  }
  for (const key of approved.keys())
    if (!matched.has(key)) violations.push(`stale-allowlist:${key.split('\0')[0]}`);
  return violations;
}

function git(args) {
  return execFileSync('git', args, { cwd: root, encoding: 'utf8', maxBuffer: 32 * 1024 * 1024 });
}

function parseTree(source) {
  return source
    .split(/\r?\n/u)
    .filter(Boolean)
    .map((line) => {
      const match = /^(\d+) blob ([0-9a-f]{40,64})\t(.+)$/u.exec(line);
      if (match === null) throw new Error(`Unexpected git tree entry: ${line}`);
      if (match[1] === '120000')
        throw new Error(`Symbolic links are forbidden in the release tree: ${match[3]}`);
      return { blob: match[2], path: match[3] };
    });
}

function main() {
  const allowlist = JSON.parse(readFileSync(allowlistPath, 'utf8'));
  if (
    allowlist.schemaVersion !== 1 ||
    typeof allowlist.referenceInventoryCommit !== 'string' ||
    !Array.isArray(allowlist.entries) ||
    allowlist.entries.some(
      (entry) =>
        typeof entry.path !== 'string' ||
        !/^[0-9a-f]{40,64}$/u.test(entry.blob) ||
        !Array.isArray(entry.sourcePaths) ||
        entry.sourcePaths.length === 0 ||
        typeof entry.reason !== 'string' ||
        entry.reason.length < 20,
    )
  ) {
    throw new Error('Reference independence allowlist is malformed.');
  }
  const reference = parseTree(
    git(['ls-tree', '-r', allowlist.referenceInventoryCommit, '--', 'reference']),
  );
  if (reference.length === 0) throw new Error('Known deleted reference blob inventory is empty.');
  const trackedPaths = git(['ls-files', '-z']).split('\0').filter(Boolean);
  const current = trackedPaths.map((path) => ({
    path,
    blob: git(['hash-object', '--', path]).trim(),
  }));
  const violations = compareBlobInventory(current, reference, allowlist);
  if (violations.length > 0)
    throw new Error(`Reference independence failed: ${violations.join(', ')}`);
  const inventoryHash = createHash('sha256')
    .update(reference.map(({ blob, path }) => `${blob} ${path}\n`).join(''))
    .digest('hex');
  console.log(
    `Reference independence passed: ${current.length} tracked blobs compared with ${reference.length} deleted-reference blobs (${allowlist.entries.length} exact attributed asset exceptions; inventory ${inventoryHash}).`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
