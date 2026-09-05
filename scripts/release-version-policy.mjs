import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';

const LOCAL_RUST_PACKAGES = Object.freeze([
  'talking-quill-acceptance-signer',
  'talking-quill-common-e2e',
  'talking-quill-helper',
  'talking-quill-keyboard-core',
  'talking-quill-keyboard-owner',
  'talking-quill-owner-protocol',
  'talking-quill-windows-owner-ipc',
]);

export async function verifyCoordinatedVersions(
  repositoryRoot = resolve(import.meta.dirname, '..'),
) {
  const [root, app, workspace, common, lock, setup, setupLock] = await Promise.all([
    json(resolve(repositoryRoot, 'package.json')),
    json(resolve(repositoryRoot, 'app/package.json')),
    readFile(resolve(repositoryRoot, 'helper/Cargo.toml'), 'utf8'),
    readFile(resolve(repositoryRoot, 'helper/common-e2e/Cargo.toml'), 'utf8'),
    readFile(resolve(repositoryRoot, 'helper/Cargo.lock'), 'utf8'),
    readFile(resolve(repositoryRoot, 'installer/windows-setup/Cargo.toml'), 'utf8'),
    readFile(resolve(repositoryRoot, 'installer/windows-setup/Cargo.lock'), 'utf8'),
  ]);
  const version = root.version;
  const versions = new Map([
    ['package.json', version],
    ['app/package.json', app.version],
    ['helper/Cargo.toml', tomlVersion(workspace)],
    ['helper/common-e2e/Cargo.toml', tomlVersion(common)],
    ['installer/windows-setup/Cargo.toml', tomlVersion(setup)],
    ['installer/windows-setup/Cargo.lock', lockVersion(setupLock, 'talking-quill-windows-setup')],
  ]);
  for (const name of LOCAL_RUST_PACKAGES)
    versions.set(`helper/Cargo.lock:${name}`, lockVersion(lock, name));
  if (
    !/^\d+\.\d+\.\d+$/u.test(version ?? '') ||
    [...versions.values()].some((value) => value !== version)
  ) {
    throw new Error(
      `Coordinated release versions disagree: ${JSON.stringify(Object.fromEntries(versions))}`,
    );
  }
  return version;
}

async function json(path) {
  return JSON.parse(await readFile(path, 'utf8'));
}
function tomlVersion(source) {
  return /^version\s*=\s*"([^"]+)"/mu.exec(source)?.[1];
}
function lockVersion(source, name) {
  return new RegExp(
    `\\[\\[package\\]\\]\\r?\\nname = "${name}"\\r?\\nversion = "([^"]+)"`,
    'u',
  ).exec(source)?.[1];
}
