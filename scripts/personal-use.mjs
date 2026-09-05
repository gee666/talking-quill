import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { constants, existsSync } from 'node:fs';
import { mkdir, open, readFile, rm, unlink } from 'node:fs/promises';
import { basename, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

import {
  createFreshEnvironment,
  PERSONAL_TARGETS,
  requireHost,
  sanitizePersonalConsumerEnvironment,
  requireFreshMacAbsence,
  waitForExactMacProcesses,
} from './personal-host-policy.mjs';
import {
  WINDOWS_ELEVATION_WRAPPER,
  WINDOWS_STAGED_PATH_ENV,
  WINDOWS_STAGED_SHA256_ENV,
  withStagedWindowsInstaller,
} from './personal-windows-installer.mjs';
export {
  createFreshEnvironment,
  PERSONAL_TARGETS,
  sanitizePersonalConsumerEnvironment,
  detectPhysicalMacArchitecture,
  readMacSigningConfiguration,
  requireNotRegisteredMacStatus,
} from './personal-host-policy.mjs';
export {
  WINDOWS_ELEVATION_WRAPPER,
  removeWindowsInstallerStaging,
  withStagedWindowsInstaller,
} from './personal-windows-installer.mjs';
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
      `${stem}${configuration.platform === 'win' ? '-setup.exe' : '.zip'}`,
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
  run(process.execPath, [resolve(root, 'scripts/run-package.mjs'), configuration.packageTarget], {
    env: { ...createFreshEnvironment(configuration), npm_execpath: pnpmCli },
  });
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

export function requireMatchingArtifactSha256(expected, actual) {
  if (!/^[0-9a-f]{64}$/u.test(expected) || actual !== expected) {
    throw new Error('Package artifact changed after verification');
  }
}

function run(executable, arguments_, options = {}) {
  const result = spawnSync(executable, arguments_, {
    cwd: root,
    env: sanitizePersonalConsumerEnvironment(),
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
