import { readdir, readFile } from 'node:fs/promises';
import { extname, relative, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const ignored = new Set(['.git', 'node_modules', 'out', 'release', 'coverage', 'tmp']);
const scannedExtensions = new Set([
  '.ts',
  '.tsx',
  '.js',
  '.jsx',
  '.mjs',
  '.cjs',
  '.css',
  '.scss',
  '.json',
  '.html',
  '.yaml',
  '.yml',
]);
const exempt = new Set([
  'scripts/verify-boundaries.mjs',
  'scripts/release-audit.mjs',
  'scripts/inspect-package.mjs',
  'scripts/reference-independence-allowlist.json',
  'tests/unit/reference-independence.test.ts',
  'eslint.config.mjs',
]);
const forbiddenPath = /(?:\.\.[/\\])+reference(?:[/\\]|\b)|(?:^|["'`(])[/\\]?reference[/\\]/im;
const violations = [];

async function walk(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    if (entry.isDirectory() && ignored.has(entry.name)) continue;
    const absolute = resolve(directory, entry.name);
    if (entry.isDirectory()) {
      await walk(absolute);
      continue;
    }
    const name = relative(root, absolute).replaceAll('\\', '/');
    if (!scannedExtensions.has(extname(entry.name)) || exempt.has(name)) continue;
    const source = await readFile(absolute, 'utf8');
    if (forbiddenPath.test(source)) violations.push(name);
  }
}

await walk(root);
if (violations.length > 0) {
  throw new Error(`Forbidden reference boundary crossed by: ${violations.join(', ')}`);
}
console.log('Source, config, CSS, and asset reference boundaries verified');
