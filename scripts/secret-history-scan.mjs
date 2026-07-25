import { spawn } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { SECRET_SCAN_OVERLAP_BYTES, findSecretRuleIds } from './secret-rules.mjs';
import {
  computeSecretEvidence,
  evidenceMatches,
  validateSecretHistoryAllowlist,
} from './secret-history-schema.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const ALLOWLIST_PATH = resolve(ROOT, 'scripts/secret-history-allowlist.json');
process.chdir(ROOT);

if ((await gitText(['rev-parse', '--is-shallow-repository'])).trim() !== 'false') {
  throw new Error('Secret history scan requires a complete non-shallow clone (fetch-depth: 0).');
}
const allowlist = validateSecretHistoryAllowlist(
  JSON.parse(await readFile(ALLOWLIST_PATH, 'utf8')),
);
const allowed = new Set(allowlist.entries.map((entry) => `${entry.blobOid}:${entry.rule}`));
const objects = await readObjectInventory();
for (const entry of allowlist.entries) {
  const path = objects.get(entry.blobOid);
  if (path === undefined) throw new Error(`Stale secret allowlist blob: ${entry.blobOid}`);
  const actual = computeSecretEvidence(await gitBlob(entry.blobOid), entry.rule, path);
  if (!evidenceMatches(entry, actual))
    throw new Error(`Changed secret allowlist evidence: ${entry.blobOid}:${entry.rule}`);
}
const findings = [];
const matchedAllowed = new Set();
let bytesScanned = 0;
let blobsScanned = 0;

await scanBatch(objects, ({ oid, type, size, matched }) => {
  if (type !== 'blob') return;
  blobsScanned += 1;
  bytesScanned += size;
  for (const rule of matched) {
    const key = `${oid}:${rule}`;
    if (allowed.has(key)) matchedAllowed.add(key);
    else
      findings.push({ blob: oid, path: objects.get(oid) ?? '(historical path unavailable)', rule });
  }
});
const stale = allowlist.entries.filter(
  (entry) => !matchedAllowed.has(`${entry.blobOid}:${entry.rule}`),
);
if (stale.length > 0)
  throw new Error(
    `Stale secret allowlist entries: ${stale.map((entry) => `${entry.blobOid}:${entry.rule}`).join(', ')}`,
  );
if (findings.length > 0)
  throw new Error(
    `Potential secrets in full git history:\n${findings.map((item) => `- ${item.rule} ${item.blob} ${item.path}`).join('\n')}`,
  );
const commitCount = Number((await gitText(['rev-list', '--all', '--count'])).trim());
const classifications = Object.groupBy(allowlist.entries, (entry) => entry.classification);
console.log(
  `Full-history secret scan v3 passed: ${String(commitCount)} commits, ${String(blobsScanned)} reachable blobs, ${String(bytesScanned)} streamed bytes, ${String(allowlist.entries.length)} exact evidence entries (synthetic-fixture=${String(classifications['synthetic-fixture']?.length ?? 0)}, reviewed-nonsecret=${String(classifications['reviewed-nonsecret']?.length ?? 0)}). Advisory only: allowlisted literals remain findings and are never suppressed from evidence recomputation.`,
);

async function readObjectInventory() {
  const child = spawn('git', ['rev-list', '--objects', '--all'], {
    cwd: ROOT,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const objects = new Map();
  let pending = '';
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr = bounded(stderr + chunk);
  });
  child.stdout.setEncoding('utf8');
  child.stdout.on('data', (chunk) => {
    pending += chunk;
    for (;;) {
      const newline = pending.indexOf('\n');
      if (newline < 0) break;
      const line = pending.slice(0, newline).replace(/\r$/u, '');
      pending = pending.slice(newline + 1);
      record(line);
    }
    if (pending.length > 65_536)
      throw new Error('git object inventory emitted an overlong record.');
  });
  const status = await exitStatus(child);
  if (status !== 0) throw new Error(`git rev-list failed (${String(status)}): ${stderr}`);
  if (pending.length > 0) record(pending);
  return objects;
  function record(line) {
    if (line.length === 0) return;
    const separator = line.indexOf(' ');
    const oid = separator < 0 ? line : line.slice(0, separator);
    if (!/^[0-9a-f]{40,64}$/u.test(oid))
      throw new Error('git object inventory contained an invalid object id.');
    if (!objects.has(oid))
      objects.set(oid, separator < 0 ? '(historical path unavailable)' : line.slice(separator + 1));
  }
}

async function scanBatch(objects, consume) {
  const child = spawn('git', ['cat-file', '--batch'], {
    cwd: ROOT,
    stdio: ['pipe', 'pipe', 'pipe'],
  });
  let stderr = '';
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr = bounded(stderr + chunk);
  });
  for (const oid of objects.keys()) child.stdin.write(`${oid}\n`);
  child.stdin.end();
  let buffer = Buffer.alloc(0);
  let current = null;
  for await (const incoming of child.stdout) {
    buffer = Buffer.concat([buffer, incoming]);
    for (;;) {
      if (current === null) {
        const newline = buffer.indexOf(0x0a);
        if (newline < 0) {
          if (buffer.length > 512) throw new Error('git cat-file emitted an overlong header.');
          break;
        }
        const header = buffer.subarray(0, newline).toString('ascii');
        buffer = buffer.subarray(newline + 1);
        const [oid, type, sizeText] = header.split(' ');
        const size = Number(sizeText);
        if (
          oid === undefined ||
          type === undefined ||
          !objects.has(oid) ||
          !Number.isSafeInteger(size) ||
          size < 0
        )
          throw new Error(`git cat-file emitted an invalid header: ${header}`);
        current = {
          oid,
          type,
          size,
          remaining: size,
          overlap: Buffer.alloc(0),
          matched: new Set(),
        };
      }
      if (current.remaining > 0 && buffer.length > 0) {
        const length = Math.min(current.remaining, buffer.length, 64 * 1024);
        const chunk = buffer.subarray(0, length);
        if (current.type === 'blob') {
          const combined = Buffer.concat([current.overlap, chunk]);
          for (const id of findSecretRuleIds(combined.toString('latin1'))) current.matched.add(id);
          current.overlap = combined.subarray(
            Math.max(0, combined.length - SECRET_SCAN_OVERLAP_BYTES),
          );
        }
        buffer = buffer.subarray(length);
        current.remaining -= length;
      }
      if (current.remaining > 0) break;
      if (buffer.length === 0) break;
      if (buffer[0] !== 0x0a) throw new Error(`git cat-file truncated blob ${current.oid}.`);
      buffer = buffer.subarray(1);
      consume(current);
      current = null;
    }
  }
  const status = await exitStatus(child);
  if (status !== 0) throw new Error(`git cat-file failed (${String(status)}): ${stderr}`);
  if (current !== null || buffer.length !== 0)
    throw new Error('git cat-file output was truncated.');
}

async function gitBlob(oid) {
  const child = spawn('git', ['cat-file', 'blob', oid], {
    cwd: ROOT,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const chunks = [];
  let size = 0;
  child.stdout.on('data', (chunk) => {
    size += chunk.length;
    if (size > 64 * 1024 * 1024) child.kill();
    else chunks.push(chunk);
  });
  const status = await exitStatus(child);
  if (status !== 0) throw new Error(`Unable to read allowlisted blob ${oid}.`);
  return Buffer.concat(chunks);
}

async function gitText(args) {
  const child = spawn('git', args, { cwd: ROOT, stdio: ['ignore', 'pipe', 'pipe'] });
  const chunks = [];
  let size = 0;
  let stderr = '';
  child.stdout.on('data', (chunk) => {
    size += chunk.length;
    if (size > 1024 * 1024) child.kill();
    else chunks.push(chunk);
  });
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk) => {
    stderr = bounded(stderr + chunk);
  });
  const status = await exitStatus(child);
  if (status !== 0) throw new Error(`git ${args.join(' ')} failed (${String(status)}): ${stderr}`);
  return Buffer.concat(chunks).toString('utf8');
}
function exitStatus(child) {
  return new Promise((resolveExit, reject) => {
    child.once('error', reject);
    child.once('close', (code) => resolveExit(code));
  });
}
function bounded(value) {
  return value.slice(-65_536);
}
