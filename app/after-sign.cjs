const { join } = require('node:path');

module.exports = async function serializeWindowsReleaseIdentity(context) {
  if (context.electronPlatformName !== 'win32') return;

  console.log('  • serializing final owner-enabled Windows release layout');
  const architecture = context.arch === 1 ? 'x64' : context.arch === 3 ? 'arm64' : null;
  if (architecture === null) {
    throw new Error(`Unsupported package architecture: ${String(context.arch)}`);
  }
  const { RELEASE_PACKAGE_METADATA_NAME, writePackageReleaseMetadata } =
    await import('../scripts/release-package-metadata.mjs');
  await writePackageReleaseMetadata(
    join(context.appOutDir, 'resources', RELEASE_PACKAGE_METADATA_NAME),
    {
      version: context.packager.appInfo.version,
      platform: 'win',
      architecture,
      packageRoot: context.appOutDir,
    },
  );
};
