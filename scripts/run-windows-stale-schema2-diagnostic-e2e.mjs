import { spawnSync } from 'node:child_process';
import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseTqpkg2 } from './tqpkg2.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
if (process.platform !== 'win32') {
  throw new Error('The stale schema-2 packaged executable test requires Windows');
}
const architecture = process.argv[2] ?? 'x64';
if (!['x64', 'arm64'].includes(architecture)) {
  throw new Error('Usage: run-windows-stale-schema2-diagnostic-e2e.mjs [x64|arm64]');
}
const version = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8')).version;
const canonical = resolve(
  root,
  'release',
  `Talking-Quill-${version}-win-${architecture}-setup.exe`,
);
const diagnostic = resolve(
  root,
  'release',
  `Talking-Quill-${version}-win-${architecture}-stale-schema2-cleanup.exe`,
);
const canonicalPackage = parseTqpkg2(await readFile(canonical), architecture);
if (canonicalPackage.manifest.packageMode !== 'fresh') {
  throw new Error('canonical packaged executable is not a fresh TQPKG2 package');
}
const diagnosticPackage = parseTqpkg2(await readFile(diagnostic), architecture, {
  allowStaleSchema2Cleanup: true,
});
if (diagnosticPackage.manifest.packageMode !== 'stale-schema2-cleanup') {
  throw new Error('diagnostic packaged executable has the wrong TQPKG2 package mode');
}
const environment = { ...process.env };
delete environment.TQ_STALE_SCHEMA2_AUDIT_PATH;
delete environment.TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH;
const invoke = (path) =>
  spawnSync(path, ['/TQ-DIAGNOSE-STALE-SCHEMA2'], {
    env: environment,
    timeout: 30_000,
    windowsHide: true,
  });
const canonicalResult = invoke(canonical);
if (canonicalResult.error !== undefined || canonicalResult.status !== 64) {
  throw new Error(`canonical package did not reject the diagnostic command with exit 64`);
}
const diagnosticResult = invoke(diagnostic);
if (diagnosticResult.error !== undefined || diagnosticResult.status !== 78) {
  throw new Error(`diagnostic package did not fail closed without protected evidence with exit 78`);
}
console.log('Packaged stale schema-2 diagnostic command gates passed');
