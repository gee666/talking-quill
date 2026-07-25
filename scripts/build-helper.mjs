import { spawnSync } from 'node:child_process';
import { chmod, copyFile, mkdir, readFile, rm, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const options = parseOptions(process.argv.slice(2));
const platform = normalizePlatform(options.platform ?? process.platform);
const architecture = normalizeArchitecture(options.architecture ?? process.arch);
if (platform !== process.platform) {
  throw new Error(
    `Cannot build a ${platform} helper on ${process.platform}; use the native CI runner`,
  );
}

const target = rustTarget(platform, architecture);
const cargo = resolveRustTool('cargo');
const rustup = resolveRustTool('rustup');
await verifyVersions();
run(rustup, ['target', 'add', target]);
run(cargo, [
  'build',
  '--manifest-path',
  'helper/Cargo.toml',
  '--locked',
  '--release',
  '--target',
  target,
]);

const executableName = platform === 'win32' ? 'talking-quill-helper.exe' : 'talking-quill-helper';
const source = join(repositoryRoot, 'helper', 'target', target, 'release', executableName);
const destinationDirectory = join(repositoryRoot, 'app', 'native');
const destination = join(destinationDirectory, executableName);
const sourceMetadata = await stat(source);
if (!sourceMetadata.isFile() || sourceMetadata.size === 0) {
  throw new Error(`Rust build did not produce a non-empty regular helper: ${source}`);
}
await rm(destinationDirectory, { recursive: true, force: true });
await mkdir(destinationDirectory, { recursive: true });
await copyFile(source, destination);
if (platform === 'darwin') await chmod(destination, 0o755);
console.log(`Staged ${target} helper at ${destination}`);

function parseOptions(arguments_) {
  const parsed = {};
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === '--') continue;
    if (argument === '--platform') parsed.platform = arguments_[++index];
    else if (argument === '--arch') parsed.architecture = arguments_[++index];
    else throw new Error(`Unknown helper-build argument: ${String(argument)}`);
  }
  return parsed;
}

function normalizePlatform(value) {
  if (value === 'win' || value === 'win32') return 'win32';
  if (value === 'mac' || value === 'darwin') return 'darwin';
  throw new Error(`Unsupported helper platform: ${String(value)}`);
}

function normalizeArchitecture(value) {
  if (value === 'x64' || value === 'arm64') return value;
  throw new Error(`Unsupported helper architecture: ${String(value)}`);
}

function rustTarget(targetPlatform, targetArchitecture) {
  const cpu = targetArchitecture === 'x64' ? 'x86_64' : 'aarch64';
  return targetPlatform === 'win32' ? `${cpu}-pc-windows-msvc` : `${cpu}-apple-darwin`;
}

function resolveRustTool(name) {
  const executable = process.platform === 'win32' ? `${name}.exe` : name;
  const cargoHome = process.env.CARGO_HOME ?? join(homedir(), '.cargo');
  const candidate = join(cargoHome, 'bin', executable);
  return existsSync(candidate) ? candidate : executable;
}

function run(command, arguments_) {
  const result = spawnSync(command, arguments_, {
    cwd: repositoryRoot,
    env: process.env,
    stdio: 'inherit',
    windowsHide: true,
  });
  if (result.error !== undefined) throw result.error;
  if (result.status !== 0) {
    throw new Error(
      `${basename(command)} ${arguments_.join(' ')} failed with ${String(result.status)}`,
    );
  }
}

async function packageVersion(path) {
  const manifest = JSON.parse(await readFile(path, 'utf8'));
  if (typeof manifest.version !== 'string') throw new Error(`Missing version in ${path}`);
  return manifest.version;
}

async function verifyVersions() {
  const [rootVersion, appVersion, cargoManifest] = await Promise.all([
    packageVersion(join(repositoryRoot, 'package.json')),
    packageVersion(join(repositoryRoot, 'app', 'package.json')),
    readFile(join(repositoryRoot, 'helper', 'Cargo.toml'), 'utf8'),
  ]);
  const helperVersion = /^version\s*=\s*"([^"]+)"/m.exec(cargoManifest)?.[1];
  if (helperVersion === undefined || rootVersion !== appVersion || appVersion !== helperVersion) {
    throw new Error(
      `Application/helper version mismatch (root=${rootVersion}, app=${appVersion}, helper=${String(helperVersion)})`,
    );
  }
}
