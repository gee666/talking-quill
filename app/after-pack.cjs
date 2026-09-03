const { existsSync, readdirSync } = require('node:fs');
const { chmod, lstat, readFile, writeFile } = require('node:fs/promises');
const { createHash } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { extname, join, relative, resolve, sep } = require('node:path');
const { flipFuses, FuseVersion, FuseV1Options } = require('@electron/fuses');
const {
  assertNoForbiddenProductionMarkers,
} = require('../scripts/forbidden-production-markers.cjs');

module.exports = async function hardenElectron(context) {
  console.log('  • verifying bundled native helper');
  const helper =
    context.electronPlatformName === 'darwin'
      ? join(
          context.appOutDir,
          `${context.packager.appInfo.productFilename}.app`,
          'Contents',
          'Resources',
          'helper',
          'talking-quill-helper',
        )
      : join(context.appOutDir, 'resources', 'helper', 'talking-quill-helper.exe');
  const metadata = await lstat(helper);
  if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size === 0) {
    throw new Error(`Bundled helper is not a non-empty regular file: ${helper}`);
  }
  if (context.electronPlatformName === 'darwin') await chmod(helper, 0o755);
  await verifyArchitecture(helper, context.electronPlatformName, context.arch);
  if (context.electronPlatformName === 'win32') {
    const architecture = context.arch === 1 ? 'x64' : context.arch === 3 ? 'arm64' : null;
    if (architecture === null) {
      throw new Error(`Unsupported package architecture: ${String(context.arch)}`);
    }
    const helperDirectory = join(context.appOutDir, 'resources', 'helper');
    const { nativeRoleLayout, verifyNativeSourceIdentity, verifyStagedNativeRoleSet } =
      await import('../scripts/helper-build-contract.mjs');
    const { currentSourceIdentity } = await import('../scripts/source-identity.mjs');
    await verifyStagedNativeRoleSet(helperDirectory, {
      platform: 'win32',
      architecture,
    });
    const sourceIdentity = currentSourceIdentity();
    await Promise.all(
      nativeRoleLayout('win32').map((role) =>
        verifyNativeSourceIdentity(join(helperDirectory, role.name), sourceIdentity),
      ),
    );
  }

  console.log('  • hardening Electron fuses');
  const product = context.packager.appInfo.productFilename;
  const executable =
    context.electronPlatformName === 'darwin'
      ? join(context.appOutDir, `${product}.app`, 'Contents', 'MacOS', product)
      : join(
          context.appOutDir,
          `${product}${context.electronPlatformName === 'win32' ? '.exe' : ''}`,
        );

  await flipFuses(executable, {
    version: FuseVersion.V1,
    [FuseV1Options.RunAsNode]: false,
    [FuseV1Options.EnableCookieEncryption]: true,
    [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
    [FuseV1Options.EnableNodeCliInspectArguments]: false,
    [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
    [FuseV1Options.OnlyLoadAppFromAsar]: true,
  });

  if (
    ['canonical', 'directory-test'].includes(
      process.env.TALKING_QUILL_PACKAGE_VARIANT ?? 'canonical',
    )
  ) {
    await scanCanonicalRuntime(context, executable);
  }
  await writeWindowsAcceptanceManifest(context, executable);
  await verifyPackagedStructure(context);
};

async function scanCanonicalRuntime(context, executable, additionalPaths = []) {
  const resources = join(
    context.appOutDir,
    context.electronPlatformName === 'darwin'
      ? `${context.packager.appInfo.productFilename}.app/Contents/Resources`
      : 'resources',
  );
  const paths = [executable, join(resources, 'app.asar'), ...additionalPaths];
  for (const root of [join(resources, 'helper'), join(resources, 'app.asar.unpacked')]) {
    await collectNativeFiles(root, paths);
  }
  for (const path of paths) {
    assertNoForbiddenProductionMarkers(path, await readFile(path));
  }
}

module.exports.scanCanonicalRuntime = scanCanonicalRuntime;

async function collectNativeFiles(directory, paths) {
  if (!existsSync(directory)) return;
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) await collectNativeFiles(path, paths);
    else if (
      entry.isFile() &&
      ['.dll', '.dylib', '.exe', '.node', ''].includes(extname(entry.name))
    ) {
      paths.push(path);
    }
  }
}

async function writeWindowsAcceptanceManifest(context, executable) {
  const resources = join(context.appOutDir, 'resources');
  const output = join(resources, 'windows-installed-acceptance-v1.txt');
  const acceptance = process.env.TALKING_QUILL_ACCEPTANCE_BUILD === '1';
  if (!acceptance) {
    if (existsSync(output)) throw new Error('Canonical package contains acceptance authorization');
    return;
  }
  if (context.electronPlatformName !== 'win32') {
    throw new Error('Installed acceptance builds are Windows-only');
  }
  const unsignedPayloadPath = process.env.TALKING_QUILL_ACCEPTANCE_UNSIGNED_MANIFEST_PAYLOAD_PATH;
  if (unsignedPayloadPath === undefined) {
    throw new Error('Acceptance builds require an external narrow signing subprocess');
  }
  const requestPublicKeySpkiBase64url =
    process.env.TALKING_QUILL_ACCEPTANCE_REQUEST_PUBLIC_KEY_SPKI_BASE64URL ?? '';
  const buildId = process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID ?? '';
  const validFromMs = Number(process.env.TALKING_QUILL_ACCEPTANCE_VALID_FROM_MS);
  const validUntilMs = Number(process.env.TALKING_QUILL_ACCEPTANCE_VALID_UNTIL_MS);
  if (
    !/^[A-Za-z0-9_-]+$/u.test(requestPublicKeySpkiBase64url) ||
    !/^[0-9a-f]{64}$/u.test(buildId) ||
    !Number.isSafeInteger(validFromMs) ||
    !Number.isSafeInteger(validUntilMs)
  ) {
    throw new Error('Acceptance build authorization inputs are invalid');
  }
  const ownerManifestPath = join(resources, 'keyboard-owner-release-v1.json');
  const ownerManifestBytes = await readFile(ownerManifestPath);
  const ownerManifest = JSON.parse(ownerManifestBytes.toString('utf8'));
  const role = (name) => ownerManifest.roles.find((value) => value.role === name);
  const gateway = role('gateway');
  const owner = role('owner');
  if (!gateway || !owner) throw new Error('Acceptance owner manifest roles are missing');
  if (validUntilMs <= validFromMs || validUntilMs - validFromMs > 31 * 24 * 60 * 60 * 1_000) {
    throw new Error('Acceptance build validity must be positive and no longer than 31 days');
  }
  const payload = {
    version: 1,
    purpose: 'talking-quill/installed-acceptance-build',
    sourceRevision: execFileSync('git', ['rev-parse', '--short=12', 'HEAD'], {
      encoding: 'utf8',
    }).trim(),
    buildId,
    architecture: context.arch === 1 ? 'x64' : context.arch === 3 ? 'arm64' : 'invalid',
    packageVersion: context.packager.appInfo.version,
    releaseBuildDigest: ownerManifest.releaseBuildDigest,
    packageLayoutDigest: ownerManifest.packageLayoutDigest,
    ownerManifestSha256: hash(ownerManifestBytes),
    electronSha256: hash(await readFile(executable)),
    appAsarSha256: hash(await readFile(join(resources, 'app.asar'))),
    gatewaySha256: hash(await readFile(join(context.appOutDir, gateway.path))),
    ownerSha256: hash(await readFile(join(context.appOutDir, owner.path))),
    requestPublicKeySpkiBase64url,
    validFromMs,
    validUntilMs,
  };
  const payloadPath = resolve(unsignedPayloadPath);
  const isolatedRoot = resolve(process.cwd(), 'tmp');
  const payloadRelative = relative(isolatedRoot, payloadPath);
  if (
    payloadRelative === '' ||
    payloadRelative === '..' ||
    payloadRelative.startsWith(`..${sep}`) ||
    payloadRelative.includes(':')
  ) {
    throw new Error('Unsigned acceptance payload must stay below the isolated build root');
  }
  await writeFile(payloadPath, `${canonicalJson(payload)}\n`, { mode: 0o600 });
  if (existsSync(output)) {
    throw new Error('Unsigned acceptance build retained a signed bearer from an earlier build');
  }
}

function canonicalJson(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean')
    return JSON.stringify(value);
  if (typeof value === 'number') {
    if (!Number.isSafeInteger(value)) throw new Error('Acceptance manifest number is invalid');
    return String(value);
  }
  if (Array.isArray(value)) return `[${value.map(canonicalJson).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
    .join(',')}}`;
}

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

async function verifyPackagedStructure(context) {
  const { verifyPackagedAsarStructure } = await import('../scripts/packaged-asar-structure.mjs');
  await verifyPackagedAsarStructure(context);
}

async function verifyArchitecture(executable, platform, arch) {
  const expected = arch === 1 ? 'x64' : arch === 3 ? 'arm64' : null;
  if (expected === null) throw new Error(`Unsupported package architecture: ${String(arch)}`);
  const bytes = await readFile(executable);
  const actual = platform === 'darwin' ? readMachArchitecture(bytes) : readPeArchitecture(bytes);
  if (actual !== expected) {
    throw new Error(`Bundled helper architecture ${actual} does not match package ${expected}`);
  }
}

function readPeArchitecture(bytes) {
  if (bytes.length < 64 || bytes.readUInt16LE(0) !== 0x5a4d) return 'invalid';
  const peOffset = bytes.readUInt32LE(0x3c);
  if (peOffset + 6 > bytes.length || bytes.readUInt32LE(peOffset) !== 0x00004550) return 'invalid';
  const machine = bytes.readUInt16LE(peOffset + 4);
  if (machine === 0x8664) return 'x64';
  if (machine === 0xaa64) return 'arm64';
  return 'unsupported';
}

function readMachArchitecture(bytes) {
  if (bytes.length < 8 || bytes.readUInt32LE(0) !== 0xfeedfacf) return 'invalid';
  const cpu = bytes.readUInt32LE(4);
  if (cpu === 0x01000007) return 'x64';
  if (cpu === 0x0100000c) return 'arm64';
  return 'unsupported';
}
