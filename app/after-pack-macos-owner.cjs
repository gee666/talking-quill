const hardenElectron = require('./after-pack.cjs');
const { execFile } = require('node:child_process');
const { chmod, copyFile, mkdir, readFile, writeFile } = require('node:fs/promises');
const { join, resolve } = require('node:path');
const { promisify } = require('node:util');
const execFileAsync = promisify(execFile);

module.exports = async function afterPackMacosOwner(context) {
  await hardenElectron(context);
  if (context.electronPlatformName !== 'darwin') {
    throw new Error('The macOS owner package hook cannot run for another platform');
  }
  const product = context.packager.appInfo.productFilename;
  const app = join(context.appOutDir, `${product}.app`);
  const nested = join(app, 'Contents', 'Library', 'LoginItems', 'Talking Quill Keyboard Owner.app');
  const contents = join(nested, 'Contents');
  const macos = join(contents, 'MacOS');
  const resources = join(contents, 'Resources');
  await mkdir(macos, { recursive: true });
  await mkdir(resources, { recursive: true });
  const ownerSource = resolve(__dirname, 'native', 'talking-quill-keyboard-owner');
  const ownerTarget = join(macos, 'talking-quill-keyboard-owner');
  await copyFile(ownerSource, ownerTarget);
  await chmod(ownerTarget, 0o755);
  const bridgeSource = resolve(__dirname, 'native', 'talking-quill-macos-service-bridge');
  const bridgeTarget = join(app, 'Contents', 'MacOS', 'talking-quill-macos-service-bridge');
  await copyFile(bridgeSource, bridgeTarget);
  await chmod(bridgeTarget, 0o755);
  const denialAddon = join(app, 'Contents', 'Resources', 'macos-keychain-denial.node');
  const packageArch = process.env.TALKING_QUILL_PACKAGE_ARCH;
  const clangArch = packageArch === 'x64' ? 'x86_64' : packageArch === 'arm64' ? 'arm64' : null;
  if (clangArch === null) throw new Error('The macOS denial addon requires one exact package arch');
  await execFileAsync('/usr/bin/clang', [
    '-arch',
    clangArch,
    '-bundle',
    '-undefined',
    'dynamic_lookup',
    '-framework',
    'CoreFoundation',
    '-framework',
    'Security',
    resolve(__dirname, '..', 'build', 'macos-keychain-denial-addon.c'),
    '-o',
    denialAddon,
  ]);
  await chmod(denialAddon, 0o755);
  await writeFile(
    join(app, 'Contents', 'Resources', 'keyboard-owner-installed-v1'),
    'talking-quill-keyboard-owner-v1\n',
    { mode: 0o644 },
  );
  const version = context.packager.appInfo.version;
  const template = await readFile(
    resolve(__dirname, '..', 'build', 'macos-keyboard-owner', 'Info.plist'),
    'utf8',
  );
  await writeFile(join(contents, 'Info.plist'), template.replaceAll('${VERSION}', version), {
    mode: 0o644,
  });
  if ((process.env.TALKING_QUILL_PACKAGE_VARIANT ?? 'canonical') === 'canonical') {
    await hardenElectron.scanCanonicalRuntime(context, join(app, 'Contents', 'MacOS', product), [
      ownerTarget,
      bridgeTarget,
      denialAddon,
    ]);
  }
};
