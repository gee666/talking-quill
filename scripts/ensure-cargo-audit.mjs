import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';
import { CARGO_AUDIT_VERSION, isExpectedCargoAuditVersion } from './security-tool-versions.mjs';
const cargo = tool('cargo');
const audit = spawnSync(cargo, ['audit', '--version'], { encoding: 'utf8' });
if (audit.status === 0 && isExpectedCargoAuditVersion(audit.stdout)) {
  console.log(`cargo-audit ${CARGO_AUDIT_VERSION} is already installed.`);
  process.exit(0);
}
const install = spawnSync(
  cargo,
  ['install', 'cargo-audit', '--version', CARGO_AUDIT_VERSION, '--locked', '--force'],
  { stdio: 'inherit' },
);
if (install.status !== 0)
  throw new Error(`Unable to install pinned cargo-audit ${CARGO_AUDIT_VERSION}.`);
const verified = spawnSync(cargo, ['audit', '--version'], { encoding: 'utf8' });
if (verified.status !== 0 || !isExpectedCargoAuditVersion(verified.stdout)) {
  throw new Error(`Pinned cargo-audit verification failed: ${verified.stdout.trim()}`);
}
console.log(`Installed and verified cargo-audit ${CARGO_AUDIT_VERSION}.`);

function tool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const candidate = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}
