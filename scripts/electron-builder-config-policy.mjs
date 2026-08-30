import { createRequire } from 'node:module';
import { dirname, isAbsolute, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { validateElectronBuilderOnnxConfig } from './package-policy.mjs';

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const { getConfig } = electronBuilderRequire('app-builder-lib/out/util/config/config.js');

const packageConfigs = Object.freeze([
  'electron-builder.yml',
  'electron-builder.unsigned.yml',
  'electron-builder.macos-owner.yml',
  'electron-builder.installed-acceptance.yml',
  'electron-builder.packaged-test.yml',
]);
const targets = Object.freeze([
  { platform: 'win', architecture: 'x64' },
  { platform: 'win', architecture: 'arm64' },
  { platform: 'mac', architecture: 'x64' },
  { platform: 'mac', architecture: 'arm64' },
]);

export async function loadMergedElectronBuilderConfig(configPath) {
  const resolvedConfig = isAbsolute(configPath) ? configPath : resolve(root, configPath);
  return getConfig(resolve(root, 'app'), resolvedConfig, null);
}

export async function validateElectronBuilderConfigFile(configPath) {
  const config = await loadMergedElectronBuilderConfig(configPath);
  for (const target of targets) validateElectronBuilderOnnxConfig(config, target);
}

export async function validatePackageElectronBuilderConfigs() {
  for (const name of packageConfigs) {
    await validateElectronBuilderConfigFile(resolve(root, 'build', name));
  }
}
