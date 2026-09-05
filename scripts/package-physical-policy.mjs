import { assertSafePaths, normalizePackagePath } from './package-path-policy.mjs';
import { ONNX_RUNTIME_PATHS } from './package-onnx-policy.mjs';

const ONNX_RESOURCE_PREFIX = 'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3';
const ONNX_RESOURCE_PATHS = Object.freeze(
  Object.fromEntries(
    [
      ['win', 'win32'],
      ['mac', 'darwin'],
    ].map(([target, platform]) => [
      target,
      Object.freeze(
        ONNX_RUNTIME_PATHS.filter((entry) =>
          entry.startsWith(`node_modules/onnxruntime-node/bin/napi-v3/${platform}`),
        ).map((entry) => `app.asar.unpacked/${entry}`),
      ),
    ]),
  ),
);
const COMMON_RESOURCE_PATHS = [
  'app-update.yml',
  'app.asar',
  'LICENSE',
  'THIRD_PARTY_NOTICES.txt',
  'app-icon.png',
  'app.asar.unpacked',
  'app.asar.unpacked/node_modules',
  'app.asar.unpacked/node_modules/better-sqlite3',
  'app.asar.unpacked/node_modules/better-sqlite3/build',
  'app.asar.unpacked/node_modules/better-sqlite3/build/Release',
  'app.asar.unpacked/node_modules/better-sqlite3/build/Release/better_sqlite3.node',
  'app.asar.unpacked/node_modules/onnxruntime-node',
  'app.asar.unpacked/node_modules/onnxruntime-node/bin',
  ONNX_RESOURCE_PREFIX,
  'helper',
];
const WINDOWS_PERSONAL_RUNTIME_RESOURCES = Object.freeze([
  'helper/talking-quill-helper.exe',
  'helper/talking-quill-keyboard-owner.exe',
  'helper/talking-quill-update-recovery-launcher.exe',
]);
const RELEASE_PACKAGE_METADATA_PATH = 'keyboard-owner-release-v1.json';
const PLATFORM_RESOURCE_PATHS = Object.freeze({
  win: Object.freeze([
    'elevate.exe',
    RELEASE_PACKAGE_METADATA_PATH,
    ...WINDOWS_PERSONAL_RUNTIME_RESOURCES,
  ]),
  mac: Object.freeze(['electron.icns', 'icon.icns', 'helper/talking-quill-helper']),
});

const WINDOWS_PHYSICAL_EXACT = new Set([
  'Talking Quill.exe',
  'chrome_100_percent.pak',
  'chrome_200_percent.pak',
  'd3dcompiler_47.dll',
  'dxcompiler.dll',
  'dxil.dll',
  'ffmpeg.dll',
  'icudtl.dat',
  'libEGL.dll',
  'libGLESv2.dll',
  'LICENSE.electron.txt',
  'LICENSES.chromium.html',
  'locales',
  'resources',
  'resources.pak',
  'snapshot_blob.bin',
  'v8_context_snapshot.bin',
  'vk_swiftshader.dll',
  'vk_swiftshader_icd.json',
  'vulkan-1.dll',
]);
const WINDOWS_LOCALE_PATTERN = /^locales\/[A-Za-z]{2,3}(?:-[A-Za-z0-9]{2,3})?\.pak$/u;
const MAC_LOCALIZATION_DIRECTORY_PATTERN = /^[a-z]{2,3}(?:_[A-Z]{2}|_419)?\.lproj$/u;
const MAC_FRAMEWORK_PATTERN =
  /^Talking Quill\.app\/Contents\/Frameworks\/(?:Electron Framework\.framework|Mantle\.framework|ReactiveObjC\.framework|Squirrel\.framework|Sparkle\.framework|Talking Quill Helper(?: \(GPU\)| \(Plugin\)| \(Renderer\))?\.app)(?:\/.*)?$/u;
const MAC_OWNER_BUNDLE_PREFIX =
  'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app';
const MAC_OWNER_PHYSICAL_EXACT = new Set([
  'Talking Quill.app/Contents/Library',
  'Talking Quill.app/Contents/Library/LoginItems',
  MAC_OWNER_BUNDLE_PREFIX,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/Info.plist`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/MacOS`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/MacOS/talking-quill-keyboard-owner`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/Resources`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/_CodeSignature`,
  `${MAC_OWNER_BUNDLE_PREFIX}/Contents/_CodeSignature/CodeResources`,
  'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
]);
const MAC_OWNER_RESOURCE_PATHS = Object.freeze([
  'keyboard-owner-r5m.json',
  'keyboard-owner-installed-v1',
  'macos-keychain-denial.node',
  RELEASE_PACKAGE_METADATA_PATH,
]);
const MAC_PHYSICAL_EXACT = new Set([
  '.background',
  '.background/background.tiff',
  '.background.tiff',
  '.DS_Store',
  '.VolumeIcon.icns',
  'Applications',
  'Talking Quill.app',
  'Talking Quill.app/Contents',
  'Talking Quill.app/Contents/Frameworks',
  'Talking Quill.app/Contents/Info.plist',
  'Talking Quill.app/Contents/MacOS',
  'Talking Quill.app/Contents/MacOS/Talking Quill',
  'Talking Quill.app/Contents/PkgInfo',
  'Talking Quill.app/Contents/Resources',
  'Talking Quill.app/Contents/_CodeSignature',
  'Talking Quill.app/Contents/_CodeSignature/CodeResources',
]);

export function validatePhysicalEntries(entries) {
  assertSafePaths(entries.map(normalizePackagePath));
}

export function validatePhysicalPackageEntries(entries, target, options = {}) {
  const normalized = entries.map(normalizePackagePath);
  assertSafePaths(normalized);
  const unexpected = normalized.filter((entry) => {
    if (target === 'win') {
      return (
        !WINDOWS_PHYSICAL_EXACT.has(entry) &&
        !WINDOWS_LOCALE_PATTERN.test(entry) &&
        !entry.startsWith('resources/')
      );
    }
    const nestedOwner = options.macosOwner === true && MAC_OWNER_PHYSICAL_EXACT.has(entry);
    return (
      !MAC_PHYSICAL_EXACT.has(entry) &&
      !nestedOwner &&
      !entry.startsWith('Talking Quill.app/Contents/Resources/') &&
      !MAC_FRAMEWORK_PATTERN.test(entry)
    );
  });
  if (unexpected.length > 0) {
    throw new Error(`Unexpected physical package entries: ${unexpected.join(', ')}`);
  }
}

export function validateResourceEntries(entries, target, options = {}) {
  if (!['win', 'mac'].includes(target)) {
    throw new Error(`Unknown packaged resource target: ${String(target)}`);
  }
  const normalized = entries.map(normalizePackagePath);
  const targetArchitecture = options.architecture;
  if (targetArchitecture !== undefined && !['x64', 'arm64'].includes(targetArchitecture)) {
    throw new Error(`Unknown packaged resource architecture: ${String(targetArchitecture)}`);
  }
  const targetOnnxResources =
    targetArchitecture === undefined
      ? ONNX_RESOURCE_PATHS[target]
      : ONNX_RESOURCE_PATHS[target].filter(
          (entry) =>
            !entry.startsWith(
              `${ONNX_RESOURCE_PREFIX}/${target === 'mac' ? 'darwin' : 'win32'}/`,
            ) ||
            entry ===
              `${ONNX_RESOURCE_PREFIX}/${target === 'mac' ? 'darwin' : 'win32'}/${targetArchitecture}` ||
            entry.startsWith(
              `${ONNX_RESOURCE_PREFIX}/${target === 'mac' ? 'darwin' : 'win32'}/${targetArchitecture}/`,
            ),
        );
  const allowed = new Set([
    ...COMMON_RESOURCE_PATHS,
    ...(options.macosOwner === true ? MAC_OWNER_RESOURCE_PATHS : []),
    ...(options.windowsInstalledAcceptance === true ? ['windows-installed-acceptance-v1.txt'] : []),
    ...PLATFORM_RESOURCE_PATHS[target],
    ...targetOnnxResources,
  ]);
  assertSafePaths(normalized);
  const unexpected = normalized.filter(
    (entry) =>
      entry.length > 0 &&
      !allowed.has(entry) &&
      !(target === 'mac' && MAC_LOCALIZATION_DIRECTORY_PATTERN.test(entry)),
  );
  if (unexpected.length > 0) {
    throw new Error(`Unexpected packaged resources: ${unexpected.join(', ')}`);
  }
  const helpers =
    target === 'mac' ? ['helper/talking-quill-helper'] : WINDOWS_PERSONAL_RUNTIME_RESOURCES;
  for (const required of [
    'app.asar',
    'LICENSE',
    'THIRD_PARTY_NOTICES.txt',
    'app.asar.unpacked/node_modules/better-sqlite3/build/Release/better_sqlite3.node',
    ONNX_RESOURCE_PREFIX,
    ...helpers,
    ...(target === 'win' ? [RELEASE_PACKAGE_METADATA_PATH] : []),
    ...(options.windowsInstalledAcceptance === true ? ['windows-installed-acceptance-v1.txt'] : []),
    ...(options.macosOwner === true ? MAC_OWNER_RESOURCE_PATHS : []),
  ]) {
    if (!normalized.includes(required)) {
      throw new Error(`Required packaged resource is missing: ${required}`);
    }
  }
  const onnxPlatform = target === 'mac' ? 'darwin' : 'win32';
  const platformRoot = `${ONNX_RESOURCE_PREFIX}/${onnxPlatform}`;
  const architectures = ['x64', 'arm64'].filter((arch) =>
    normalized.includes(`${platformRoot}/${arch}`),
  );
  if (architectures.length !== 1) {
    throw new Error(
      `Required ONNX resource architecture count is not one: ${architectures.length}`,
    );
  }
  if (targetArchitecture !== undefined && architectures[0] !== targetArchitecture) {
    throw new Error(
      `Required ONNX resource architecture is ${architectures[0]}, expected ${targetArchitecture}`,
    );
  }
  const architectureRoot = `${platformRoot}/${architectures[0]}`;
  for (const required of ONNX_RESOURCE_PATHS[target].filter(
    (entry) => entry === platformRoot || entry.startsWith(`${architectureRoot}/`),
  )) {
    if (!normalized.includes(required)) {
      throw new Error(`Required ONNX resource path is missing: ${required}`);
    }
  }
}
