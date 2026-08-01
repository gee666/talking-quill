'use strict';

const { execFileSync } = require('node:child_process');
const { createHash, randomBytes } = require('node:crypto');
const { existsSync, readFileSync, readdirSync } = require('node:fs');
const { mkdir, readFile, rm, writeFile } = require('node:fs/promises');
const { createRequire } = require('node:module');
const path = require('node:path');
const { fileURLToPath } = require('node:url');
const { signAsync } = require('@electron/osx-sign');

const RUST_HELPER_RELATIVE_PATH = 'Contents/Resources/helper/talking-quill-helper';
const repositoryRoot = path.resolve(__dirname, '..');
const signingDirectory = path.resolve(repositoryRoot, 'tmp', 'prebuilt-signing');
const entitlementDigests = new Map([
  ['entitlements.mac.plist', '9c4721fbddf5ad203e04b8be01fb39ecad83e4436cb99292c4640647d5b8cd29'],
  [
    'entitlements.mac.inherit.plist',
    '388be301001eccb288edca1c9f0deb89285ef94bff6d26ab436275b82b41d1a0',
  ],
]);
const rootRequire = createRequire(path.resolve(repositoryRoot, 'package.json'));
const electronBuilderRequire = createRequire(rootRequire.resolve('electron-builder/package.json'));
const appBuilderRequire = createRequire(
  electronBuilderRequire.resolve('app-builder-lib/package.json'),
);

function loadBuilderDependency(name) {
  // Use the signer versions owned by the pinned electron-builder toolchain rather than resolving a
  // second, potentially divergent implementation in the credentialed release stage.
  return appBuilderRequire(name);
}

function isRustHelper(appPath, filePath) {
  const relative = path
    .relative(path.resolve(appPath), path.resolve(filePath))
    .split(path.sep)
    .join('/');
  return relative === RUST_HELPER_RELATIVE_PATH;
}

function verifyEntitlements(entitlements) {
  if (entitlements === undefined || (Array.isArray(entitlements) && entitlements.length === 0)) {
    return;
  }
  if (typeof entitlements !== 'string') {
    throw new Error('Signed files must use one of the reviewed entitlement files.');
  }
  const name = path.basename(entitlements);
  const expected = entitlementDigests.get(name);
  if (expected === undefined) throw new Error(`Unreviewed entitlement file: ${entitlements}`);
  const entitlementPath = path.isAbsolute(entitlements)
    ? entitlements
    : path.resolve(repositoryRoot, 'build', entitlements);
  const actual = createHash('sha256').update(readFileSync(entitlementPath)).digest('hex');
  if (actual !== expected) throw new Error(`Reviewed entitlement policy changed: ${name}`);
}

function optionsForSignedFile(configuration, filePath) {
  const inherited = configuration.optionsForFile?.(filePath) ?? {};
  if (isRustHelper(configuration.app, filePath)) {
    return { ...inherited, entitlements: [], hardenedRuntime: true };
  }
  verifyEntitlements(inherited.entitlements);
  if (typeof inherited.entitlements !== 'string') return inherited;
  return {
    ...inherited,
    entitlements: path.isAbsolute(inherited.entitlements)
      ? inherited.entitlements
      : path.resolve(repositoryRoot, 'build', inherited.entitlements),
  };
}

async function signWithLeastPrivilege(configuration) {
  await signAsync({
    ...configuration,
    optionsForFile: (filePath) => optionsForSignedFile(configuration, filePath),
  });
}

function containedRepositoryPath(input) {
  const resolvedPath = path.resolve(repositoryRoot, input);
  const repositoryRelativePath = path.relative(repositoryRoot, resolvedPath);
  if (
    repositoryRelativePath.length === 0 ||
    repositoryRelativePath === '..' ||
    repositoryRelativePath.startsWith(`..${path.sep}`) ||
    path.isAbsolute(repositoryRelativePath)
  ) {
    throw new Error(`Signing path must be a contained repository path: ${input}`);
  }
  return resolvedPath;
}

function findMacApplication(packageRoot) {
  const applications = readdirSync(packageRoot, { withFileTypes: true })
    .filter((entry) => entry.isDirectory() && entry.name.endsWith('.app'))
    .map((entry) => path.resolve(packageRoot, entry.name));
  if (applications.length !== 1) {
    throw new Error(
      `Expected exactly one prebuilt macOS application; found ${applications.length}`,
    );
  }
  return applications[0];
}

async function certificateBytes(link) {
  const trimmed = link.trim();
  if (trimmed.length === 0) throw new Error('CSC_LINK is empty.');

  if (trimmed.startsWith('data:')) {
    const match = /^data:[^,]*;base64,([A-Za-z0-9+/=\s]+)$/u.exec(trimmed);
    if (match?.[1] === undefined) throw new Error('CSC_LINK data URL must contain base64 data.');
    return decodeBase64Certificate(match[1]);
  }
  if (trimmed.startsWith('file:')) return readFile(fileURLToPath(trimmed));
  if (/^https:\/\//u.test(trimmed)) {
    const response = await fetch(trimmed, { signal: AbortSignal.timeout(30_000) });
    if (!response.ok) throw new Error(`Certificate download failed with HTTP ${response.status}.`);
    const bytes = Buffer.from(await response.arrayBuffer());
    validateCertificateSize(bytes, 'Downloaded');
    return bytes;
  }

  const localPath = path.resolve(repositoryRoot, trimmed);
  if (existsSync(localPath)) return readFile(localPath);
  return decodeBase64Certificate(trimmed);
}

function decodeBase64Certificate(value) {
  const compact = value.replaceAll(/\s/gu, '');
  if (
    compact.length === 0 ||
    compact.length % 4 === 1 ||
    !/^[A-Za-z0-9+/]*={0,2}$/u.test(compact)
  ) {
    throw new Error('CSC_LINK must be an HTTPS URL, file URL, repository path, or valid base64.');
  }
  const bytes = Buffer.from(compact.padEnd(Math.ceil(compact.length / 4) * 4, '='), 'base64');
  validateCertificateSize(bytes, 'Decoded');
  return bytes;
}

function validateCertificateSize(bytes, source) {
  if (bytes.length === 0 || bytes.length > 2 * 1024 * 1024) {
    throw new Error(`${source} certificate has an invalid size.`);
  }
}

function requiredEnvironment(name) {
  const value = process.env[name]?.trim();
  if (!value) throw new Error(`${name} is required.`);
  return value;
}

function run(command, arguments_) {
  execFileSync(command, arguments_, { stdio: 'inherit' });
}

function notarizationOptions(appPath) {
  return {
    tool: 'notarytool',
    appPath,
    appleId: requiredEnvironment('APPLE_ID'),
    appleIdPassword: requiredEnvironment('APPLE_APP_SPECIFIC_PASSWORD'),
    teamId: requiredEnvironment('APPLE_TEAM_ID'),
  };
}

async function signWindows(packageRoot) {
  const certificatePath = path.resolve(signingDirectory, 'windows-certificate.pfx');
  await mkdir(signingDirectory, { recursive: true });
  await writeFile(certificatePath, await certificateBytes(requiredEnvironment('CSC_LINK')), {
    mode: 0o600,
  });
  try {
    const { sign } = loadBuilderDependency('@electron/windows-sign');
    await sign({
      appDirectory: packageRoot,
      certificateFile: certificatePath,
      certificatePassword: requiredEnvironment('CSC_KEY_PASSWORD'),
      description: 'Talking Quill',
      hashes: ['sha256'],
    });
  } finally {
    await rm(signingDirectory, { recursive: true, force: true });
  }
}

async function signMac(packageRoot) {
  const app = findMacApplication(packageRoot);
  const certificatePath = path.resolve(signingDirectory, 'macos-certificate.p12');
  const keychainPath = path.resolve(signingDirectory, 'talking-quill.keychain-db');
  const keychainPassword = randomBytes(32).toString('hex');
  await mkdir(signingDirectory, { recursive: true });
  await writeFile(certificatePath, await certificateBytes(requiredEnvironment('CSC_LINK')), {
    mode: 0o600,
  });
  try {
    run('security', ['create-keychain', '-p', keychainPassword, keychainPath]);
    run('security', ['set-keychain-settings', '-lut', '21600', keychainPath]);
    run('security', ['unlock-keychain', '-p', keychainPassword, keychainPath]);
    run('security', [
      'import',
      certificatePath,
      '-k',
      keychainPath,
      '-P',
      requiredEnvironment('CSC_KEY_PASSWORD'),
      '-T',
      '/usr/bin/codesign',
      '-T',
      '/usr/bin/security',
    ]);
    run('security', [
      'set-key-partition-list',
      '-S',
      'apple-tool:,apple:',
      '-s',
      '-k',
      keychainPassword,
      keychainPath,
    ]);

    const appEntitlements = path.resolve(repositoryRoot, 'build', 'entitlements.mac.plist');
    const inheritedEntitlements = path.resolve(
      repositoryRoot,
      'build',
      'entitlements.mac.inherit.plist',
    );
    await signWithLeastPrivilege({
      app,
      keychain: keychainPath,
      platform: 'darwin',
      type: 'distribution',
      strictVerify: true,
      preAutoEntitlements: false,
      preEmbedProvisioningProfile: false,
      optionsForFile: (filePath) => ({
        entitlements: path.resolve(filePath) === app ? appEntitlements : inheritedEntitlements,
        hardenedRuntime: true,
      }),
    });
    const { notarize } = loadBuilderDependency('@electron/notarize');
    await notarize(notarizationOptions(app));
  } finally {
    try {
      run('security', ['delete-keychain', keychainPath]);
    } catch {
      // Preserve the original signing failure while still removing local key material below.
    }
    await rm(signingDirectory, { recursive: true, force: true });
  }
}

async function notarizeDiskImage(input) {
  const diskImage = containedRepositoryPath(input);
  if (!diskImage.endsWith('.dmg') || !existsSync(diskImage)) {
    throw new Error(`Expected a produced DMG below the repository root: ${input}`);
  }
  const { notarize } = loadBuilderDependency('@electron/notarize');
  await notarize(notarizationOptions(diskImage));
}

async function main(arguments_) {
  const [command, targetOrPath, packageRootInput] = arguments_;
  if (command === 'sign' && (targetOrPath === 'win' || targetOrPath === 'mac')) {
    if (packageRootInput === undefined) throw new Error('A prebuilt package root is required.');
    const packageRoot = containedRepositoryPath(packageRootInput);
    if (targetOrPath === 'win') await signWindows(packageRoot);
    else await signMac(packageRoot);
    return;
  }
  if (command === 'notarize-dmg' && targetOrPath !== undefined && packageRootInput === undefined) {
    await notarizeDiskImage(targetOrPath);
    return;
  }
  throw new Error(
    'Usage: node build/mac-sign.cjs sign <win|mac> <package-root> | notarize-dmg <path>',
  );
}

if (require.main === module) {
  main(process.argv.slice(2)).catch((error) => {
    console.error(error instanceof Error ? error.message : error);
    process.exitCode = 1;
  });
}

module.exports = signWithLeastPrivilege;
module.exports.certificateBytes = certificateBytes;
module.exports.containedRepositoryPath = containedRepositoryPath;
module.exports.findMacApplication = findMacApplication;
module.exports.isRustHelper = isRustHelper;
module.exports.notarizationOptions = notarizationOptions;
module.exports.optionsForSignedFile = optionsForSignedFile;
module.exports.RUST_HELPER_RELATIVE_PATH = RUST_HELPER_RELATIVE_PATH;
module.exports.verifyEntitlements = verifyEntitlements;
