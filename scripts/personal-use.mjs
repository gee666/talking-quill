import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, existsSync, readFileSync } from 'node:fs';
import { mkdir, mkdtemp, open, readFile, rm, unlink } from 'node:fs/promises';
import { homedir } from 'node:os';
import { basename, resolve } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';
import { fileURLToPath } from 'node:url';

import {
  RELEASE_PACKAGE_METADATA_NAME,
  verifyMatchingPackageReleaseMetadataBytes,
  verifySerializedPackageReleaseMetadata,
} from './release-package-metadata.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const manifest = JSON.parse(await readFile(resolve(root, 'app/package.json'), 'utf8'));
const version = manifest.version;
const command = process.argv[2];
const target = process.argv[3];

const WINDOWS_STAGED_PATH_ENV = 'TALKING_QUILL_PERSONAL_STAGED_INSTALLER';
const WINDOWS_STAGED_SHA256_ENV = 'TALKING_QUILL_PERSONAL_STAGED_SHA256';
const WINDOWS_STAGING_CLEANUP_ATTEMPTS = 10;
const PERSONAL_PRODUCER_ENVIRONMENT = new Set([
  'TALKING_QUILL_NATIVE_FAULT_PHASE',
  'TALKING_QUILL_PACKAGE_MODE',
  'TALKING_QUILL_PACKAGE_VARIANT',
  'TALKING_QUILL_PERSONAL_FRESH_INSTALL',
  'TALKING_QUILL_WINDOWS_FRESH_TRUST_ROOT',
]);

export const WINDOWS_ELEVATION_WRAPPER = [
  '$ErrorActionPreference="Stop"',
  '$stream=$null',
  '$hasher=$null',
  'try{',
  '$path=[Environment]::GetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_INSTALLER","Process")',
  '$expected=[Environment]::GetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_SHA256","Process")',
  'if([string]::IsNullOrWhiteSpace($path)-or[string]::IsNullOrWhiteSpace($expected)-or$expected-cnotmatch "^[0-9a-f]{64}$"){exit 70}',
  '[Environment]::SetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_INSTALLER",$null,"Process")',
  '[Environment]::SetEnvironmentVariable("TALKING_QUILL_PERSONAL_STAGED_SHA256",$null,"Process")',
  '$stream=[IO.File]::Open($path,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)',
  '$hasher=[Security.Cryptography.SHA256]::Create()',
  '$actual=-join($hasher.ComputeHash($stream)|ForEach-Object{$_.ToString("x2")})',
  '$hasher.Dispose()',
  '$hasher=$null',
  'if($actual-cne$expected){exit 70}',
  '$process=Start-Process -FilePath $path -PassThru -ErrorAction Stop',
  'if($null-eq$process){exit 70}',
  '$process.WaitForExit()',
  'exit $process.ExitCode',
  '}catch{exit 70}',
  'finally{if($null-ne$hasher){$hasher.Dispose()};if($null-ne$stream){$stream.Dispose()}}',
].join('\n');

export const PERSONAL_TARGETS = Object.freeze({
  win: { packageTarget: 'win-unsigned', platform: 'win', architecture: 'x64' },
  'win-arm64': { packageTarget: 'win-arm64-unsigned', platform: 'win', architecture: 'arm64' },
  'mac-x64': { packageTarget: 'mac-owner-x64', platform: 'mac', architecture: 'x64' },
  'mac-arm64': { packageTarget: 'mac-owner-arm64', platform: 'mac', architecture: 'arm64' },
});

export function sanitizePersonalConsumerEnvironment(source = process.env) {
  return Object.fromEntries(
    Object.entries(source).filter(
      ([name]) =>
        !PERSONAL_PRODUCER_ENVIRONMENT.has(name) &&
        !/^TALKING_QUILL_(?:MACOS_)?PREDECESSOR_/u.test(name) &&
        !/^TALKING_QUILL_.*(?:TEST|HARNESS|FIXTURE|ACCEPTANCE)/u.test(name) &&
        !/^TALKING_QUILL_.*(?:PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u.test(name),
    ),
  );
}

export function createFreshEnvironment(configuration, source = process.env) {
  const environment = sanitizePersonalConsumerEnvironment(source);
  if (configuration.platform === 'mac') {
    Object.assign(environment, readMacSigningConfiguration(source));
  }
  environment.TALKING_QUILL_PACKAGE_MODE = 'fresh';
  environment.TALKING_QUILL_PERSONAL_FRESH_INSTALL = '1';
  return environment;
}

export function packagePaths(configuration) {
  const stem = `Talking-Quill-${version}-${configuration.platform}-${configuration.architecture}`;
  const packageRoot =
    configuration.platform === 'win'
      ? resolve(
          root,
          'release',
          configuration.architecture === 'x64' ? 'win-unpacked' : 'win-arm64-unpacked',
        )
      : resolve(root, 'release', configuration.architecture === 'x64' ? 'mac' : 'mac-arm64');
  return {
    packageRoot,
    metadata: resolve(
      packageRoot,
      configuration.platform === 'win'
        ? `resources/${RELEASE_PACKAGE_METADATA_NAME}`
        : `Talking Quill.app/Contents/Resources/${RELEASE_PACKAGE_METADATA_NAME}`,
    ),
    installer: resolve(
      root,
      'release',
      `${stem}.${configuration.platform === 'win' ? 'exe' : 'zip'}`,
    ),
  };
}

async function main() {
  if (!Object.hasOwn(PERSONAL_TARGETS, target ?? '')) usage();
  const configuration = PERSONAL_TARGETS[target];
  if (command === 'build') return build(configuration);
  if (command === 'check') return check(configuration);
  if (command === 'install') return install(configuration);
  if (command === 'verify') return verify(configuration);
  usage();
}

function build(configuration) {
  requireHost(configuration, false);
  const pnpmCli = process.env.npm_execpath;
  if (!pnpmCli) throw new Error('Run this command through pnpm');
  run(
    process.execPath,
    [pnpmCli, 'exec', 'node', 'scripts/run-package.mjs', configuration.packageTarget],
    {
      env: createFreshEnvironment(configuration),
    },
  );
}

async function check(configuration) {
  const consumerEnvironment = sanitizePersonalConsumerEnvironment();
  const paths = packagePaths(configuration);
  if (!existsSync(paths.installer)) throw new Error(`Missing package artifact: ${paths.installer}`);
  const [metadataBytes, artifactBytes] = await Promise.all([
    readFile(paths.metadata),
    readFile(paths.installer),
  ]);
  const sha256 = createHash('sha256').update(artifactBytes).digest('hex');
  const pnpmCli = process.env.npm_execpath;
  if (!pnpmCli) throw new Error('Run this command through pnpm');
  run(
    process.execPath,
    [
      pnpmCli,
      'exec',
      'node',
      'scripts/inspect-package.mjs',
      paths.packageRoot,
      ...(configuration.platform === 'mac' ? ['--macos-owner'] : []),
    ],
    {
      env: {
        ...consumerEnvironment,
        TALKING_QUILL_PACKAGE_INSPECTION_STRICT: '1',
        TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED:
          configuration.platform === 'win' ? 'native-setup' : 'dmg-zip',
        TALKING_QUILL_PACKAGE_TARGET: configuration.platform,
        TALKING_QUILL_PACKAGE_ARCH: configuration.architecture,
      },
    },
  );
  const metadata = await verifySerializedPackageReleaseMetadata(paths.metadata, paths.packageRoot, {
    version,
    platform: configuration.platform,
    architecture: configuration.architecture,
  });
  verifyMatchingPackageReleaseMetadataBytes(metadataBytes, await readFile(paths.metadata));
  requireMatchingArtifactSha256(
    sha256,
    createHash('sha256')
      .update(await readFile(paths.installer))
      .digest('hex'),
  );
  if (
    metadata.packageMode !== 'fresh' ||
    metadata.freshInstall !== true ||
    metadata.predecessor !== null
  ) {
    throw new Error('Package is not the requested fresh personal-use target');
  }
  console.log(
    `Verified fresh ${configuration.platform}/${configuration.architecture} package: ${paths.installer}\nSHA-256: ${sha256}`,
  );
  return { metadata, metadataBytes, artifactBytes, sha256 };
}

async function install(configuration) {
  requireHost(configuration, true);
  const consumerEnvironment = sanitizePersonalConsumerEnvironment();
  const checked = await check(configuration);
  if (configuration.platform === 'win') {
    await withStagedWindowsInstaller(checked, (stagedInstaller, expectedSha256) => {
      run(
        'powershell.exe',
        ['-NoProfile', '-NonInteractive', '-Command', WINDOWS_ELEVATION_WRAPPER],
        {
          env: {
            ...consumerEnvironment,
            [WINDOWS_STAGED_PATH_ENV]: stagedInstaller,
            [WINDOWS_STAGED_SHA256_ENV]: expectedSha256,
          },
        },
      );
    });
    return;
  }
  if (existsSync('/Applications/Talking Quill.app')) {
    throw new Error(
      'Fresh installation refuses to replace /Applications/Talking Quill.app; use an exact predecessor-bound update/rollback or uninstall first',
    );
  }
  const staging = resolve(root, 'tmp', 'personal-use-install');
  await rm(staging, { recursive: true, force: true });
  await mkdir(staging, { recursive: true, mode: 0o700 });
  const stagedZip = resolve(staging, 'checked-package.zip');
  const stagedHandle = await open(
    stagedZip,
    constants.O_CREAT | constants.O_EXCL | constants.O_RDWR | constants.O_NOFOLLOW,
    0o600,
  );
  const extraction = resolve(staging, 'extracted');
  try {
    await unlink(stagedZip);
    await stagedHandle.writeFile(checked.artifactBytes);
    await stagedHandle.sync();
    run('/usr/bin/ditto', ['-x', '-k', '/dev/fd/3', extraction], {
      env: consumerEnvironment,
      stdio: ['inherit', 'inherit', 'inherit', stagedHandle.fd],
    });
  } finally {
    await stagedHandle.close();
  }
  const app = resolve(extraction, 'Talking Quill.app');
  const stagedMetadataPath = resolve(app, 'Contents', 'Resources', RELEASE_PACKAGE_METADATA_NAME);
  const stagedMetadata = await verifySerializedPackageReleaseMetadata(
    stagedMetadataPath,
    extraction,
    { version, platform: 'mac', architecture: configuration.architecture },
  );
  if (stagedMetadata.freshInstall !== true || stagedMetadata.predecessor !== null) {
    throw new Error('Extracted ZIP is not the verified fresh package');
  }
  verifyMatchingPackageReleaseMetadataBytes(
    checked.metadataBytes,
    await readFile(stagedMetadataPath),
  );
  run('/usr/bin/codesign', ['--verify', '--deep', '--strict', app], {
    env: consumerEnvironment,
  });
  requireFreshMacAbsence(
    resolve(app, 'Contents', 'MacOS', 'talking-quill-macos-service-bridge'),
    consumerEnvironment,
  );
  run('/usr/bin/sudo', ['/usr/bin/ditto', app, '/Applications/Talking Quill.app'], {
    env: consumerEnvironment,
  });
  console.log('Installed /Applications/Talking Quill.app. Control-click it and choose Open.');
}

async function verify(configuration) {
  requireHost(configuration, true);
  const consumerEnvironment = sanitizePersonalConsumerEnvironment();
  const checked = await check(configuration);
  if (configuration.platform === 'win') {
    const installedRoot = resolve(
      process.env.ProgramW6432 ?? process.env.ProgramFiles ?? '',
      'Talking Quill',
    );
    const installedMetadataPath = resolve(
      installedRoot,
      'resources',
      RELEASE_PACKAGE_METADATA_NAME,
    );
    verifyMatchingPackageReleaseMetadataBytes(
      checked.metadataBytes,
      await readFile(installedMetadataPath),
    );
    await verifySerializedPackageReleaseMetadata(installedMetadataPath, installedRoot, {
      version,
      platform: 'win',
      architecture: configuration.architecture,
    });
    const service = spawnSync('sc.exe', ['query', 'TalkingQuillKeyboardAuthority'], {
      encoding: 'utf8',
      env: consumerEnvironment,
    });
    if (service.status === 0) {
      throw new Error('Legacy TalkingQuillKeyboardAuthority service still exists');
    }
    run(
      process.execPath,
      [
        'tests/native/helper-harness.mjs',
        '--helper',
        resolve(installedRoot, 'resources/helper/talking-quill-helper.exe'),
      ],
      { env: consumerEnvironment },
    );
    console.log('Installed gateway and owner authenticated without an SCM dependency.');
    return;
  }
  if (process.arch !== configuration.architecture) {
    throw new Error(
      `Installed verification requires native ${configuration.architecture}; this host is ${process.arch}`,
    );
  }
  const installedApp = '/Applications/Talking Quill.app';
  const installedMetadataPath = resolve(
    installedApp,
    'Contents',
    'Resources',
    RELEASE_PACKAGE_METADATA_NAME,
  );
  verifyMatchingPackageReleaseMetadataBytes(
    checked.metadataBytes,
    await readFile(installedMetadataPath),
  );
  await verifySerializedPackageReleaseMetadata(installedMetadataPath, '/Applications', {
    version,
    platform: 'mac',
    architecture: configuration.architecture,
  });
  for (const executable of [
    `${installedApp}/Contents/MacOS/Talking Quill`,
    `${installedApp}/Contents/Resources/helper/talking-quill-helper`,
    `${installedApp}/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner`,
    `${installedApp}/Contents/MacOS/talking-quill-macos-service-bridge`,
  ]) {
    const result = spawnSync('/usr/bin/lipo', ['-archs', executable], {
      encoding: 'utf8',
      env: consumerEnvironment,
    });
    const nativeArch = configuration.architecture === 'x64' ? 'x86_64' : 'arm64';
    if (result.status !== 0 || result.stdout.trim() !== nativeArch) {
      throw new Error(`Installed executable architecture mismatch: ${executable}`);
    }
  }
  run('/usr/bin/open', [installedApp], { env: consumerEnvironment });
  waitForExactMacProcesses(
    [
      `${installedApp}/Contents/Resources/helper/talking-quill-helper`,
      `${installedApp}/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner`,
    ],
    consumerEnvironment,
  );
  run(
    '/Applications/Talking Quill.app/Contents/Resources/helper/talking-quill-helper',
    ['--macos-owner-validate-install'],
    { env: consumerEnvironment },
  );
  console.log(
    'Gateway and Keyboard Owner are running and the installed policy validates. Perform the physical capture check in docs/personal-use-install.md.',
  );
}

export function readMacSigningConfiguration(source = process.env) {
  const path =
    source.TALKING_QUILL_MACOS_LOCAL_CONFIG ??
    resolve(
      homedir(),
      'Library',
      'Application Support',
      'Talking Quill Local Build',
      'signing.json',
    );
  if (!existsSync(path)) throw new Error(`Run pnpm personal:mac:setup first; missing ${path}`);
  const value = JSON.parse(readFileSync(path, 'utf8'));
  const required = [
    'TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256',
    'TALKING_QUILL_MACOS_POLICY_CMS_IDENTITY',
    'TALKING_QUILL_MACOS_INSTALLATION_ID',
    'TALKING_QUILL_MACOS_LOCAL_IDENTITY',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA1',
  ];
  if (
    value === null ||
    typeof value !== 'object' ||
    Array.isArray(value) ||
    Object.keys(value).some((name) => !required.includes(name))
  ) {
    throw new Error('macOS local configuration contains unknown fields');
  }
  for (const name of required)
    if (typeof value[name] !== 'string' || value[name] === '')
      throw new Error(`Invalid macOS local configuration: ${name}`);
  for (const name of [
    'TALKING_QUILL_MACOS_POLICY_SIGNER_SHA256',
    'TALKING_QUILL_MACOS_INSTALLATION_ID',
    'TALKING_QUILL_MACOS_LOCAL_CERT_SHA256',
  ]) {
    if (!/^[0-9a-f]{64}$/u.test(value[name]))
      throw new Error(`Invalid macOS local digest: ${name}`);
  }
  if (!/^[0-9a-f]{40}$/u.test(value.TALKING_QUILL_MACOS_LOCAL_CERT_SHA1)) {
    throw new Error('Invalid macOS local SHA-1 certificate fingerprint');
  }
  const mode = source.TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE ?? 'self-signed';
  if (!['self-signed', 'adhoc'].includes(mode)) throw new Error('Invalid local signing mode');
  return Object.fromEntries([
    ...required.map((name) => [name, value[name]]),
    ['TALKING_QUILL_MACOS_LOCAL_SIGNING_MODE', mode],
  ]);
}

export function requireMatchingArtifactSha256(expected, actual) {
  if (!/^[0-9a-f]{64}$/u.test(expected) || actual !== expected) {
    throw new Error('Package artifact changed after verification');
  }
}

export async function removeWindowsInstallerStaging(
  staging,
  remove = rm,
  wait = delay,
  attempts = WINDOWS_STAGING_CLEANUP_ATTEMPTS,
) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      await remove(staging, { recursive: true, force: true });
      return;
    } catch (error) {
      lastError = error;
      if (attempt < attempts) await wait(attempt * 100);
    }
  }
  throw new Error('Windows installer staging cleanup failed after bounded retries', {
    cause: lastError,
  });
}

export async function withStagedWindowsInstaller(
  checked,
  launch,
  stagingParent = resolve(root, 'tmp'),
  cleanup = removeWindowsInstallerStaging,
) {
  await mkdir(stagingParent, { recursive: true, mode: 0o700 });
  const staging = await mkdtemp(resolve(stagingParent, 'personal-use-win-install-'));
  const stagedInstaller = resolve(staging, 'checked-package.exe');
  let stagedHandle;
  let primaryError;
  let launchResult;
  try {
    stagedHandle = await open(
      stagedInstaller,
      constants.O_CREAT | constants.O_EXCL | constants.O_RDWR | constants.O_NOFOLLOW,
      0o600,
    );
    await stagedHandle.writeFile(checked.artifactBytes);
    await stagedHandle.sync();
    await stagedHandle.close();
    stagedHandle = undefined;
    // The launcher must verify through a retained read-only FileStream and keep
    // that same object open until the elevated installer process has exited.
    launchResult = await launch(stagedInstaller, checked.sha256);
  } catch (error) {
    primaryError = error;
  }
  await stagedHandle?.close().catch(() => undefined);
  let cleanupError;
  try {
    await cleanup(staging);
  } catch (error) {
    cleanupError = error;
  }
  if (primaryError !== undefined) {
    if (cleanupError !== undefined) {
      console.error(
        'Windows installer staging cleanup also failed; the primary install error is retained.',
      );
    }
    throw primaryError;
  }
  if (cleanupError !== undefined) throw cleanupError;
  return launchResult;
}

export function requireNotRegisteredMacStatus(status, output) {
  if (status !== 0 || output.trim() !== '0') {
    throw new Error('Fresh installation requires authoritative SMAppService notRegistered status');
  }
}

function requireFreshMacAbsence(serviceBridge, environment) {
  const service = 'com.talkingquill.app.keyboard-owner';
  for (const account of ['owner-ipc-v1', 'maintenance-latch-v1']) {
    const result = spawnSync(
      '/usr/bin/security',
      ['find-generic-password', '-s', service, '-a', account],
      { env: environment },
    );
    if (result.status === 0) {
      throw new Error(
        `Fresh installation refuses existing Keyboard Owner Keychain item: ${account}`,
      );
    }
    if (result.status !== 44) throw new Error('Could not prove Keyboard Owner Keychain absence');
  }
  const ownerRoot = resolve(
    homedir(),
    'Library',
    'Application Support',
    'Talking Quill',
    'KeyboardOwner',
  );
  if (existsSync(ownerRoot)) {
    throw new Error(`Fresh installation refuses pending owner state or cleanup: ${ownerRoot}`);
  }
  const serviceStatus = spawnSync(serviceBridge, ['status'], {
    encoding: 'utf8',
    env: environment,
  });
  requireNotRegisteredMacStatus(serviceStatus.status, serviceStatus.stdout);
  const processes = spawnSync('/bin/ps', ['-axo', 'command='], {
    encoding: 'utf8',
    env: environment,
  });
  if (
    processes.status !== 0 ||
    /talking-quill-(?:helper|keyboard-owner)|Talking Quill Keyboard Owner/u.test(processes.stdout)
  ) {
    throw new Error(
      'Fresh installation cannot prove that previous gateway/owner processes are absent',
    );
  }
}

function waitForExactMacProcesses(paths, environment) {
  const deadline = Date.now() + 30_000;
  while (Date.now() < deadline) {
    const result = spawnSync('/bin/ps', ['-axo', 'command='], {
      encoding: 'utf8',
      env: environment,
    });
    const commands = result.stdout.split('\n');
    if (
      result.status === 0 &&
      paths.every((path) =>
        commands.some((command) => command === path || command.startsWith(`${path} `)),
      )
    )
      return;
    spawnSync('/bin/sleep', ['1'], { env: environment });
  }
  throw new Error(`Timed out waiting for exact installed processes: ${paths.join(', ')}`);
}

function requireHost(configuration, requireNativeArchitecture) {
  const expected = configuration.platform === 'win' ? 'win32' : 'darwin';
  if (process.platform !== expected)
    throw new Error(
      `${configuration.platform} personal-use commands require a native ${expected} host`,
    );
  if (configuration.platform === 'win') {
    if (requireNativeArchitecture && process.arch !== configuration.architecture) {
      throw new Error(
        `Installed ${configuration.architecture} verification requires matching Windows hardware; this host is ${process.arch}`,
      );
    }
    return;
  }
  if (configuration.platform === 'mac') {
    const translated = spawnSync('/usr/sbin/sysctl', ['-in', 'sysctl.proc_translated'], {
      encoding: 'utf8',
    });
    const machine = spawnSync('/usr/bin/uname', ['-m'], { encoding: 'utf8' });
    if (machine.status !== 0) throw new Error('Cannot determine physical Mac architecture');
    const physical =
      translated.status === 0 && translated.stdout.trim() === '1'
        ? 'arm64'
        : machine.stdout.trim() === 'x86_64'
          ? 'x64'
          : machine.stdout.trim();
    if (physical !== configuration.architecture) {
      throw new Error(
        `${configuration.architecture} personal-use command requires matching physical hardware; this Mac is ${physical}`,
      );
    }
  }
}

function run(executable, arguments_, options = {}) {
  const result = spawnSync(executable, arguments_, {
    cwd: root,
    env: process.env,
    stdio: 'inherit',
    ...options,
  });
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${basename(executable)} failed with ${String(result.status)}`);
}

function usage() {
  throw new Error('Usage: personal-use.mjs <build|check|install|verify> <win|mac-x64|mac-arm64>');
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
