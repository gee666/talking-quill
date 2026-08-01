import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const pnpmCli = process.env.npm_execpath;
if (pnpmCli === undefined) {
  throw new Error('Run through the pinned package script so npm_execpath identifies pnpm.');
}
const version = spawnSync(process.execPath, [pnpmCli, '--version'], {
  cwd: ROOT,
  encoding: 'utf8',
});
if (version.status !== 0 || version.stdout.trim() !== '11.17.0') {
  throw new Error(`Production audit requires pnpm 11.17.0; received ${version.stdout.trim()}.`);
}
const result = spawnSync(
  process.execPath,
  [pnpmCli, 'audit', '--prod', '--json', '--audit-level', 'high'],
  {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  },
);
let report;
try {
  report = JSON.parse(result.stdout);
} catch {
  throw new Error(
    `pnpm audit did not return valid JSON (status ${String(result.status)}): ${result.stderr.trim()}`,
  );
}
if (result.error !== undefined || (result.status !== 0 && result.status !== 1)) {
  throw result.error ?? new Error(`pnpm audit failed with status ${String(result.status)}.`);
}
const counts = report?.metadata?.vulnerabilities;
if (
  typeof counts !== 'object' ||
  counts === null ||
  !['info', 'low', 'moderate', 'high', 'critical'].every(
    (severity) => Number.isInteger(counts[severity]) && counts[severity] >= 0,
  )
) {
  throw new Error('pnpm audit JSON omitted severity totals.');
}
const blocking = counts.high + counts.critical;
if ((result.status === 0) !== (blocking === 0)) {
  throw new Error('pnpm audit exit status disagrees with the configured high/critical threshold.');
}
console.log(
  `Node production dependency audit passed policy (pnpm 11.17.0): ${JSON.stringify(counts)}. Policy blocks high and critical; info/low/moderate are reported and non-blocking.`,
);
if (blocking > 0) {
  throw new Error(
    `Node production dependency policy rejected ${String(blocking)} high/critical advisories.`,
  );
}
