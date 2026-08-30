const { existsSync, readdirSync, rmSync } = require('node:fs');
const { chmod, lstat, readFile, writeFile } = require('node:fs/promises');
const { createHash, createPrivateKey, sign } = require('node:crypto');
const { execFileSync } = require('node:child_process');
const { join } = require('node:path');
const { flipFuses, FuseVersion, FuseV1Options } = require('@electron/fuses');

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

  console.log('  • hardening Electron fuses');
  const product = context.packager.appInfo.productFilename;
  const executable =
    context.electronPlatformName === 'darwin'
      ? join(context.appOutDir, `${product}.app`, 'Contents', 'MacOS', product)
      : join(
          context.appOutDir,
          `${product}${context.electronPlatformName === 'win32' ? '.exe' : ''}`,
        );

  pruneOnnxRuntime(context);

  await flipFuses(executable, {
    version: FuseVersion.V1,
    [FuseV1Options.RunAsNode]: false,
    [FuseV1Options.EnableCookieEncryption]: true,
    [FuseV1Options.EnableNodeOptionsEnvironmentVariable]: false,
    [FuseV1Options.EnableNodeCliInspectArguments]: false,
    [FuseV1Options.EnableEmbeddedAsarIntegrityValidation]: true,
    [FuseV1Options.OnlyLoadAppFromAsar]: true,
  });

  await writeWindowsAcceptanceManifest(context, executable);
};

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
  const privateKey = createPrivateKey(
    process.env.TALKING_QUILL_ACCEPTANCE_MANIFEST_PRIVATE_KEY_PEM ?? '',
  );
  if (
    privateKey.asymmetricKeyType !== 'ec' ||
    privateKey.asymmetricKeyDetails?.namedCurve !== 'prime256v1'
  ) {
    throw new Error('Acceptance manifest signing key must be P-256');
  }
  const requestPublicKeySpkiBase64url =
    process.env.TALKING_QUILL_ACCEPTANCE_REQUEST_PUBLIC_KEY_SPKI_BASE64URL ?? '';
  const buildId = process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID ?? '';
  const validUntilMs = Number(process.env.TALKING_QUILL_ACCEPTANCE_VALID_UNTIL_MS);
  if (
    !/^[A-Za-z0-9_-]+$/u.test(requestPublicKeySpkiBase64url) ||
    !/^[0-9a-f]{64}$/u.test(buildId) ||
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
  const validFromMs = Date.now() - 60_000;
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
  const signatureBase64url = sign('sha256', Buffer.from(canonicalJson(payload)), {
    key: privateKey,
    dsaEncoding: 'ieee-p1363',
  }).toString('base64url');
  await writeFile(
    output,
    Buffer.from(canonicalJson({ payload, signatureBase64url })).toString('base64url'),
    { mode: 0o600 },
  );
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

function pruneOnnxRuntime(context) {
  const archNames = new Map([
    [1, 'x64'],
    [3, 'arm64'],
  ]);
  const expectedArch = archNames.get(context.arch);
  const expectedPlatform = context.electronPlatformName;
  if (expectedArch === undefined || !['win32', 'darwin'].includes(expectedPlatform)) {
    throw new Error(`Unsupported ONNX package target: ${expectedPlatform}/${String(context.arch)}`);
  }
  const root = join(
    context.appOutDir,
    context.electronPlatformName === 'darwin'
      ? `${context.packager.appInfo.productFilename}.app/Contents/Resources`
      : 'resources',
    'app.asar.unpacked',
    'node_modules',
    'onnxruntime-node',
    'bin',
    'napi-v3',
  );
  if (!existsSync(root)) throw new Error(`Packaged ONNX runtime is missing: ${root}`);
  for (const platform of readdirSync(root, { withFileTypes: true })) {
    const platformPath = join(root, platform.name);
    if (!platform.isDirectory() || platform.name !== expectedPlatform) {
      rmSync(platformPath, { recursive: true, force: true });
      continue;
    }
    for (const arch of readdirSync(platformPath, { withFileTypes: true })) {
      if (!arch.isDirectory() || arch.name !== expectedArch) {
        rmSync(join(platformPath, arch.name), { recursive: true, force: true });
      }
    }
  }
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
