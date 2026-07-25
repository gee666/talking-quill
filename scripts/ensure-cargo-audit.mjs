import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join } from 'node:path';

const expectedVersion = /\b0\.22\.2\s*$/u;
const cargo = tool('cargo');
const audit = spawnSync(cargo, ['audit', '--version'], { encoding: 'utf8' });
if (audit.status === 0 && expectedVersion.test(audit.stdout.trim())) {
  console.log('cargo-audit 0.22.2 is already installed.');
  process.exit(0);
}
const install = spawnSync(
  cargo,
  ['install', 'cargo-audit', '--version', '0.22.2', '--locked', '--force'],
  { stdio: 'inherit' },
);
if (install.status !== 0) throw new Error('Unable to install pinned cargo-audit 0.22.2.');
const verified = spawnSync(cargo, ['audit', '--version'], { encoding: 'utf8' });
if (verified.status !== 0 || !expectedVersion.test(verified.stdout.trim())) {
  throw new Error(`Pinned cargo-audit verification failed: ${verified.stdout.trim()}`);
}
console.log('Installed and verified cargo-audit 0.22.2.');

function tool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const candidate = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}
