import { spawnSync } from 'node:child_process';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { join, resolve } from 'node:path';

const root = resolve(import.meta.dirname, '..');
const cargo = tool('cargo');
const targets = [
  'x86_64-pc-windows-msvc',
  'aarch64-pc-windows-msvc',
  'x86_64-apple-darwin',
  'aarch64-apple-darwin',
];
for (const target of targets) {
  const manifest = 'helper/Cargo.toml';
  const result = spawnSync(
    cargo,
    ['fetch', '--manifest-path', manifest, '--locked', '--target', target],
    {
      cwd: root,
      encoding: 'utf8',
      stdio: 'inherit',
    },
  );
  if (result.status !== 0)
    throw new Error(`Cargo dependency fetch failed for ${manifest} on ${target}.`);
}
console.log(`Fetched locked Rust dependency graphs for ${targets.join(', ')}.`);

function tool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const candidate = join(process.env.CARGO_HOME ?? join(homedir(), '.cargo'), 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}
