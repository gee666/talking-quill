import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = resolve(import.meta.dirname, '..');
const allowlistPath = resolve(root, 'scripts/reference-independence-allowlist.json');
const inventoryPath = resolve(root, 'scripts/reference-independence-inventory.json');

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
  const inventory = JSON.parse(readFileSync(inventoryPath, 'utf8'));
  if (
    inventory.schemaVersion !== 1 ||
    inventory.repository !== 'https://github.com/Mintplex-Labs/anything-llm.git' ||
    inventory.commit !== allowlist.referenceInventoryCommit ||
    !/^[0-9a-f]{64}$/u.test(inventory.inventorySha256 ?? '') ||
    !Array.isArray(inventory.entries) ||
    inventory.entries.length < 1_000 ||
    inventory.entries.some(
      (entry) =>
        typeof entry.path !== 'string' ||
        !entry.path.startsWith('reference/') ||
        !/^[0-9a-f]{40}$/u.test(entry.blob ?? ''),
    )
  ) {
    throw new Error('Complete pinned deleted-reference inventory is malformed.');
  }
  const reference = inventory.entries;
  const computedInventoryHash = createHash('sha256')
    .update(reference.map(({ blob, path }) => `${blob} ${path}\n`).join(''))
    .digest('hex');
  if (computedInventoryHash !== inventory.inventorySha256) {
    throw new Error('Complete pinned deleted-reference inventory digest does not match its bytes.');
  }
  const indexedPaths = git(['ls-files', '--cached', '-z']).split('\0').filter(Boolean);
  const indexedPlans = indexedPaths.filter((path) => path.startsWith('.agent-plans/'));
  if (indexedPlans.length > 0) {
    throw new Error(`Tracked agent plans are forbidden: ${indexedPlans.join(', ')}`);
  }
  const untrackedPaths = git(['ls-files', '--others', '--exclude-standard', '-z'])
    .split('\0')
    .filter((path) => path.length > 0 && !path.startsWith('.agent-plans/'));
  const trackedPaths = [...new Set([...indexedPaths, ...untrackedPaths])].filter((path) =>
    existsSync(resolve(root, path)),
  );
  const current = trackedPaths.map((path) => ({
    path,
    blob: git(['hash-object', '--', path]).trim(),
  }));
  const violations = compareBlobInventory(current, reference, allowlist);
  if (violations.length > 0)
    throw new Error(`Reference independence failed: ${violations.join(', ')}`);
  console.log(
    `Reference independence passed: ${current.length} tracked blobs compared with ${reference.length} blobs from pinned ${inventory.commit} (${allowlist.entries.length} exact attributed asset exceptions; inventory ${computedInventoryHash}).`,
  );
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) main();
