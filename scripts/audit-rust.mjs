import { spawnSync } from 'node:child_process';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { CARGO_AUDIT_VERSION, isExpectedCargoAuditVersion } from './security-tool-versions.mjs';

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const version = spawnSync('cargo', ['audit', '--version'], { cwd: ROOT, encoding: 'utf8' });
if (version.status !== 0 || !isExpectedCargoAuditVersion(version.stdout)) {
  throw new Error(
    `RustSec audit requires cargo-audit ${CARGO_AUDIT_VERSION}. Install with: cargo install cargo-audit --version ${CARGO_AUDIT_VERSION} --locked`,
  );
}
auditLockfile('helper/Cargo.lock');
console.log(
  `RustSec audit passed for the helper lock (cargo-audit ${CARGO_AUDIT_VERSION}, deny warnings, no ignores): 0 vulnerabilities and 0 warnings.`,
);

function auditLockfile(lockfile) {
  const result = spawnSync('cargo', ['audit', '--file', lockfile, '--deny', 'warnings', '--json'], {
    cwd: ROOT,
    encoding: 'utf8',
    maxBuffer: 64 * 1024 * 1024,
  });
  let report;
  try {
    report = JSON.parse(result.stdout);
  } catch {
    throw new Error(
      `cargo audit did not return valid JSON for ${lockfile} (status ${String(result.status)}): ${result.stderr.trim()}`,
    );
  }
  if (result.error !== undefined) throw result.error;
  if (!Number.isInteger(report?.vulnerabilities?.count) || report.vulnerabilities.count < 0) {
    throw new Error(`cargo audit JSON omitted the vulnerability count for ${lockfile}.`);
  }
  const warningCounts = Object.fromEntries(
    Object.entries(report.warnings ?? {}).map(([kind, entries]) => [
      kind,
      Array.isArray(entries) ? entries.length : -1,
    ]),
  );
  if (Object.values(warningCounts).some((count) => count < 0)) {
    throw new Error(`cargo audit JSON contained an invalid warning category for ${lockfile}.`);
  }
  if (Array.isArray(report?.settings?.ignore) && report.settings.ignore.length > 0) {
    throw new Error('RustSec advisory ignores are forbidden by policy.');
  }
  if (result.status !== 0) {
    throw new Error(
      `RustSec policy rejected ${lockfile} (status ${String(result.status)}): ${String(report.vulnerabilities.count)} vulnerabilities, warnings ${JSON.stringify(warningCounts)}.`,
    );
  }
  if (
    report.vulnerabilities.count !== 0 ||
    Object.values(warningCounts).some((count) => count > 0)
  ) {
    throw new Error(`cargo audit returned success despite policy findings for ${lockfile}.`);
  }
}
