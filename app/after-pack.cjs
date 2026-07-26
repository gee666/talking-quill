const { existsSync, readdirSync, rmSync } = require('node:fs');
const { chmod, lstat, readFile } = require('node:fs/promises');
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
};

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
