import { spawnSync } from 'node:child_process';
import { chmod, copyFile, mkdir, readFile, rm, stat } from 'node:fs/promises';
import { existsSync } from 'node:fs';
import { homedir } from 'node:os';
import { basename, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  nativeRoleLayout,
  verifyNativeSourceIdentity,
  verifyStagedNativeRoleSet,
} from './helper-build-contract.mjs';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { replaceNativeRoleDirectory } from './native-staging.mjs';
import { windowsUpdatePublicKeyIdentity } from './release-package-metadata.mjs';
import { currentSourceIdentity } from './source-identity.mjs';
import { verifyCoordinatedVersions } from './release-version-policy.mjs';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const options = parseOptions(process.argv.slice(2));
const sourceIdentity = currentSourceIdentity({ repositoryRoot });
process.env.TALKING_QUILL_SOURCE_COMMIT = sourceIdentity.sourceCommit;
process.env.TALKING_QUILL_SOURCE_TREE = sourceIdentity.sourceTree;
const hostEnvironment = sanitizedSubprocessEnvironment();
const platform = normalizePlatform(options.platform ?? process.platform);
const architecture = normalizeArchitecture(options.architecture ?? process.arch);
const acceptanceBuildEnvironment = 'TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD';
const acceptanceBuildValue = process.env[acceptanceBuildEnvironment];
if (
  acceptanceBuildValue !== undefined &&
  acceptanceBuildValue !== '' &&
  acceptanceBuildValue !== '1'
) {
  throw new Error(`${acceptanceBuildEnvironment} must be exactly 1 when set`);
}
if (acceptanceBuildValue === '1' && platform !== 'win32') {
  throw new Error(`${acceptanceBuildEnvironment} is valid only for Windows helper builds`);
}
const gatewayFeatures =
  acceptanceBuildValue === '1' && platform === 'win32' ? ['windows-installed-acceptance'] : [];
if (platform !== process.platform) {
  throw new Error(
    `Cannot build a ${platform} helper on ${process.platform}; use the native CI runner`,
  );
}
if (platform === 'darwin') {
  const translated = spawnSync('/usr/sbin/sysctl', ['-in', 'sysctl.proc_translated'], {
    encoding: 'utf8',
    env: hostEnvironment,
  });
  const machine = spawnSync('/usr/bin/uname', ['-m'], {
    encoding: 'utf8',
    env: hostEnvironment,
  });
  if (machine.status !== 0) throw new Error('Cannot determine physical Mac architecture');
  const physical =
    translated.status === 0 && translated.stdout.trim() === '1'
      ? 'arm64'
      : machine.stdout.trim() === 'x86_64'
        ? 'x64'
        : machine.stdout.trim();
  if (architecture !== physical) {
    throw new Error(
      `Cannot package a ${architecture} macOS helper on ${physical} hardware; use the matching native runner`,
    );
  }
}

if (platform === 'win32') windowsUpdatePublicKeyIdentity();
const target = rustTarget(platform, architecture);
const cargo = resolveRustTool('cargo');
const rustup = resolveRustTool('rustup');
await verifyCoordinatedVersions(repositoryRoot);
run(rustup, ['target', 'add', target]);

// Build each trust role explicitly. The gateway package has no owner feature;
// only the detached owner receives the enabled local-unsigned feature.
buildCargoRole('talking-quill-helper', 'talking-quill-helper', gatewayFeatures);
if (platform === 'win32') {
  buildCargoRole('talking-quill-helper', 'talking-quill-update-recovery-launcher', [
    'windows-update-recovery-launcher',
  ]);
}
const macosOwnerFeatures = ['local-unsigned-owner'];
if (
  platform === 'darwin' &&
  options.macosLifecycleFixture === true &&
  process.env.TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE === 'permissioned-ci-v1'
) {
  macosOwnerFeatures.push('macos-native-lifecycle-fixture');
}
buildCargoRole('talking-quill-keyboard-owner', 'talking-quill-keyboard-owner', macosOwnerFeatures);
if (platform === 'darwin') {
  buildCargoRole('talking-quill-helper', 'talking-quill-macos-service-bridge');
}

const destinationDirectory = join(repositoryRoot, 'app', 'native');
const stagingDirectory = join(repositoryRoot, 'app', `.native-staging-${process.pid}`);
await rm(stagingDirectory, { recursive: true, force: true });
await mkdir(stagingDirectory, { recursive: true });
try {
  for (const role of nativeRoleLayout(platform)) {
    await stage(
      join(repositoryRoot, 'helper', 'target', target, 'release', role.name),
      join(stagingDirectory, role.name),
    );
  }
  // Verify all bytes before replacing the previous coherent role set.
  await verifyStagedNativeRoleSet(stagingDirectory, { platform, architecture });
  await Promise.all(
    nativeRoleLayout(platform).map((role) =>
      verifyNativeSourceIdentity(join(stagingDirectory, role.name), sourceIdentity),
    ),
  );
  await replaceNativeRoleDirectory({
    appDirectory: join(repositoryRoot, 'app'),
    stagingDirectory,
    platform,
    architecture,
  });
} finally {
  await rm(stagingDirectory, { recursive: true, force: true });
}
console.log(`Staged verified ${target} local owner roles at ${destinationDirectory}`);

function buildCargoRole(packageName, binaryName, features = []) {
  const arguments_ = [
    'build',
    '--manifest-path',
    'helper/Cargo.toml',
    '--locked',
    '--release',
    '--target',
    target,
    '-p',
    packageName,
    '--no-default-features',
    '--bin',
    binaryName,
  ];
  if (features.length > 0) arguments_.push('--features', features.join(','));
  run(cargo, arguments_);
}

async function stage(from, to) {
  const metadata = await stat(from);
  if (!metadata.isFile() || metadata.size === 0) throw new Error(`Missing native role: ${from}`);
  await copyFile(from, to);
  if (platform === 'win32') {
    const bytes = await readFile(to);
    for (const marker of [
      'TQ_MACHINE_LOCK_TEST_NAMESPACE_ID',
      'Talking Quill Tests',
      'TalkingQuill.Tests.',
    ]) {
      if (
        bytes.includes(Buffer.from(marker, 'ascii')) ||
        bytes.includes(Buffer.from(marker, 'utf16le'))
      ) {
        throw new Error(`Windows helper contains machine-lock test marker: ${marker}`);
      }
    }
  }
  if (platform === 'darwin') await chmod(to, 0o755);
}

function parseOptions(arguments_) {
  const parsed = {};
  for (let index = 0; index < arguments_.length; index += 1) {
    const argument = arguments_[index];
    if (argument === '--') continue;
    if (argument === '--platform') parsed.platform = arguments_[++index];
    else if (argument === '--arch') parsed.architecture = arguments_[++index];
    // Retained as a compatibility no-op: R9 always stages the enabled owner.
    else if (argument === '--include-macos-owner') parsed.includeMacosOwner = true;
    else if (argument === '--macos-lifecycle-fixture') parsed.macosLifecycleFixture = true;
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
  const environment = sanitizedSubprocessEnvironment(process.env, {
    TALKING_QUILL_SOURCE_COMMIT: sourceIdentity.sourceCommit,
    TALKING_QUILL_SOURCE_TREE: sourceIdentity.sourceTree,
    ...(acceptanceBuildValue === '1' ? { [acceptanceBuildEnvironment]: '1' } : {}),
    ...(options.macosLifecycleFixture === true &&
    process.env.TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE === 'permissioned-ci-v1'
      ? { TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE: 'permissioned-ci-v1' }
      : {}),
  });
  const result = spawnSync(command, arguments_, {
    cwd: repositoryRoot,
    env: environment,
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
