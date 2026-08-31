import { createRequire } from 'node:module';
import { posix } from 'node:path';
import { findSecretRuleIds } from './secret-rules.mjs';

const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const { minimatch } = electronBuilderRequire('minimatch');

const FORBIDDEN_PARTS = [
  'reference',
  'anythingllm',
  'prisma',
  'vectordb',
  '__tests__',
  'test_extension',
  'task6-test-composition',
  'node_modules/.cache',
];
const FORBIDDEN_EXTENSIONS = new Set([
  '.c',
  '.cc',
  '.cpp',
  '.h',
  '.hpp',
  '.gyp',
  '.iobj',
  '.ipdb',
  '.lib',
  '.map',
  '.obj',
  '.pdb',
  '.ts',
  '.tsx',
  '.vcxproj',
]);

const ASAR_EXACT_FILES = new Set([
  'package.json',
  'node_modules',
  'node_modules/better-sqlite3',
  'node_modules/better-sqlite3/lib',
  'node_modules/better-sqlite3/build',
  'node_modules/better-sqlite3/build/Release',
  'node_modules/bindings',
  'node_modules/file-uri-to-path',
  'node_modules/onnxruntime-node',
  'node_modules/onnxruntime-node/dist',
  'node_modules/onnxruntime-node/bin',
  'node_modules/onnxruntime-node/bin/napi-v3',
  'node_modules/onnxruntime-common',
  'node_modules/onnxruntime-common/dist',
  'node_modules/onnxruntime-common/dist/cjs',
  'node_modules/better-sqlite3/package.json',
  'node_modules/better-sqlite3/LICENSE',
  'node_modules/better-sqlite3/build/Release/better_sqlite3.node',
  'node_modules/better-sqlite3/lib/index.js',
  'node_modules/better-sqlite3/lib/database.js',
  'node_modules/better-sqlite3/lib/sqlite-error.js',
  'node_modules/better-sqlite3/lib/util.js',
  'node_modules/better-sqlite3/lib/methods',
  'node_modules/better-sqlite3/lib/methods/aggregate.js',
  'node_modules/better-sqlite3/lib/methods/backup.js',
  'node_modules/better-sqlite3/lib/methods/function.js',
  'node_modules/better-sqlite3/lib/methods/inspect.js',
  'node_modules/better-sqlite3/lib/methods/pragma.js',
  'node_modules/better-sqlite3/lib/methods/serialize.js',
  'node_modules/better-sqlite3/lib/methods/table.js',
  'node_modules/better-sqlite3/lib/methods/transaction.js',
  'node_modules/better-sqlite3/lib/methods/wrappers.js',
  'node_modules/bindings/package.json',
  'node_modules/bindings/LICENSE.md',
  'node_modules/bindings/bindings.js',
  'node_modules/file-uri-to-path/package.json',
  'node_modules/file-uri-to-path/LICENSE',
  'node_modules/file-uri-to-path/index.js',
  'node_modules/onnxruntime-node/package.json',
  'node_modules/onnxruntime-common/package.json',
  'node_modules/onnxruntime-common/dist/cjs/package.json',
]);
export const ONNX_RUNTIME_PATHS = Object.freeze([
  'node_modules/onnxruntime-node/dist/backend.js',
  'node_modules/onnxruntime-node/dist/binding.js',
  'node_modules/onnxruntime-node/dist/index.js',
  'node_modules/onnxruntime-node/dist/version.js',
  ...[
    'backend-impl',
    'backend',
    'env-impl',
    'env',
    'index',
    'inference-session-impl',
    'inference-session',
    'onnx-model',
    'onnx-value',
    'tensor-conversion-impl',
    'tensor-conversion',
    'tensor-factory-impl',
    'tensor-factory',
    'tensor-impl-type-mapping',
    'tensor-impl',
    'tensor-utils-impl',
    'tensor-utils',
    'tensor',
    'trace',
    'type-helper',
    'version',
  ].map((name) => `node_modules/onnxruntime-common/dist/cjs/${name}.js`),
  'node_modules/onnxruntime-node/bin/napi-v3/darwin',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/arm64',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/arm64/libonnxruntime.1.21.0.dylib',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/arm64/onnxruntime_binding.node',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/x64',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/x64/libonnxruntime.1.21.0.dylib',
  'node_modules/onnxruntime-node/bin/napi-v3/darwin/x64/onnxruntime_binding.node',
  'node_modules/onnxruntime-node/bin/napi-v3/linux',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/arm64',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/arm64/libonnxruntime.so.1',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/arm64/libonnxruntime.so.1.21.0',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/arm64/onnxruntime_binding.node',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/x64',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/x64/libonnxruntime.so.1',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/x64/libonnxruntime.so.1.21.0',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/x64/libonnxruntime_providers_shared.so',
  'node_modules/onnxruntime-node/bin/napi-v3/linux/x64/onnxruntime_binding.node',
  'node_modules/onnxruntime-node/bin/napi-v3/win32',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/arm64',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/arm64/DirectML.dll',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/arm64/onnxruntime.dll',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/arm64/onnxruntime_binding.node',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/DirectML.dll',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/onnxruntime.dll',
  'node_modules/onnxruntime-node/bin/napi-v3/win32/x64/onnxruntime_binding.node',
]);
const ONNX_NATIVE_ROOT = 'node_modules/onnxruntime-node/bin/napi-v3/';
const ONNX_NATIVE_LEAF_PATTERN =
  /^node_modules\/onnxruntime-node\/bin\/napi-v3\/(?:darwin|linux|win32)\/(?:arm64|x64)\/.+/u;

function targetOnnxRuntimePaths(target) {
  if (target === undefined) return ONNX_RUNTIME_PATHS;
  if (
    !['win', 'mac'].includes(target.platform) ||
    !['x64', 'arm64'].includes(target.architecture)
  ) {
    throw new Error('ONNX runtime target must specify win|mac and x64|arm64');
  }
  const platform = target.platform === 'mac' ? 'darwin' : 'win32';
  const platformRoot = `${ONNX_NATIVE_ROOT}${platform}`;
  const architectureRoot = `${platformRoot}/${target.architecture}`;
  return ONNX_RUNTIME_PATHS.filter(
    (entry) =>
      !entry.startsWith(ONNX_NATIVE_ROOT) ||
      entry === platformRoot ||
      entry === architectureRoot ||
      entry.startsWith(`${architectureRoot}/`),
  );
}

function requiredOnnxRuntimePaths(target) {
  return targetOnnxRuntimePaths(target).filter(
    (entry) => !entry.startsWith(ONNX_NATIVE_ROOT) || ONNX_NATIVE_LEAF_PATTERN.test(entry),
  );
}

export const ONNX_BUILDER_NATIVE_INVENTORY = Object.freeze(
  ONNX_RUNTIME_PATHS.filter((entry) => ONNX_NATIVE_LEAF_PATTERN.test(entry)),
);
const ONNX_BUILDER_ROOT_EXCLUSION = `!${ONNX_NATIVE_ROOT}**/*`;
const ONNX_BUILDER_TARGET_PATTERNS = Object.freeze({
  win: Object.freeze([
    `${ONNX_NATIVE_ROOT}win32/\${arch}/DirectML.dll`,
    `${ONNX_NATIVE_ROOT}win32/\${arch}/onnxruntime.dll`,
    `${ONNX_NATIVE_ROOT}win32/\${arch}/onnxruntime_binding.node`,
  ]),
  mac: Object.freeze([
    `${ONNX_NATIVE_ROOT}darwin/\${arch}/libonnxruntime.1.21.0.dylib`,
    `${ONNX_NATIVE_ROOT}darwin/\${arch}/onnxruntime_binding.node`,
  ]),
});

export function validateElectronBuilderOnnxConfig(config, target) {
  if (
    config === null ||
    typeof config !== 'object' ||
    !['win', 'mac'].includes(target?.platform) ||
    !['x64', 'arm64'].includes(target?.architecture)
  ) {
    throw new Error('electron-builder ONNX target must specify win|mac and x64|arm64');
  }

  const platformConfig = config[target.platform];
  if (platformConfig === null || typeof platformConfig !== 'object') {
    throw new Error(`electron-builder ${target.platform} configuration is missing`);
  }

  assertOnlyOnnxBuilderPatterns(
    config.files,
    'files',
    [ONNX_BUILDER_ROOT_EXCLUSION],
    'implicit-root',
  );
  assertOnlyOnnxBuilderPatterns(config.asarUnpack, 'asarUnpack', [], null);
  const targetPatterns = ONNX_BUILDER_TARGET_PATTERNS[target.platform];
  assertOnlyOnnxBuilderPatterns(
    platformConfig.files,
    `${target.platform}.files`,
    targetPatterns,
    'explicit-root',
  );
  assertExactOnnxBuilderFileSet(platformConfig.files, `${target.platform}.files`, targetPatterns);
  assertOnlyOnnxBuilderPatterns(
    platformConfig.asarUnpack,
    `${target.platform}.asarUnpack`,
    targetPatterns,
    null,
  );

  const expectedPlatform = target.platform === 'mac' ? 'darwin' : 'win32';
  if (
    targetPatterns.some(
      (pattern) =>
        !pattern
          .replace('${arch}', target.architecture)
          .includes(`/napi-v3/${expectedPlatform}/${target.architecture}/`),
    )
  ) {
    throw new Error('electron-builder ONNX target pattern does not resolve to the target tuple');
  }
}

function assertExactOnnxBuilderFileSet(value, field, expected) {
  const candidates = Array.isArray(value) ? value : [value];
  const fileSets = candidates.filter(
    (candidate) =>
      candidate !== null &&
      typeof candidate === 'object' &&
      candidate.from === '.' &&
      candidate.to === '.' &&
      Array.isArray(candidate.filter) &&
      candidate.filter.some(
        (pattern) => typeof pattern === 'string' && isOnnxBuilderPattern(pattern),
      ),
  );
  if (
    fileSets.length !== 1 ||
    fileSets[0].filter.length !== expected.length ||
    fileSets[0].filter.some((pattern, index) => pattern !== expected[index])
  ) {
    throw new Error(`Unexpected electron-builder ONNX FileSet in ${field}`);
  }
}

function assertOnlyOnnxBuilderPatterns(value, field, expected, allowedFileSetLocation) {
  const found = [];
  collectOnnxBuilderPatterns(value, field, found, allowedFileSetLocation);
  if (
    found.length !== expected.length ||
    found.some((pattern, index) => pattern !== expected[index])
  ) {
    throw new Error(
      `Unexpected electron-builder ONNX selector in ${field}: ${found.length === 0 ? '<missing>' : found.join(', ')}`,
    );
  }
}

function collectOnnxBuilderPatterns(value, field, found, allowedFileSetLocation) {
  if (value == null) return;
  if (typeof value === 'string') {
    if (isOnnxBuilderPattern(value)) found.push(value.replaceAll('\\', '/'));
    return;
  }
  if (Array.isArray(value)) {
    for (const item of value)
      collectOnnxBuilderPatterns(item, field, found, allowedFileSetLocation);
    return;
  }
  if (typeof value !== 'object') {
    throw new Error(`electron-builder ${field} contains an unsupported matcher`);
  }

  const from = typeof value.from === 'string' ? value.from : '';
  const to = typeof value.to === 'string' ? value.to : '';
  const configuredFilters = Array.isArray(value.filter) ? value.filter : [value.filter];
  const filters = value.filter === undefined ? ['**/*'] : configuredFilters;
  const fileSetMentionsOnnx = filters.some((filter) => {
    if (typeof filter !== 'string') return false;
    const relativeFilter = filter.replace(/^!/u, '');
    const source = from === '' ? filter : `${from}/${relativeFilter}`;
    const destination = to === '' ? filter : `${to}/${relativeFilter}`;
    return (
      isOnnxBuilderPattern(source) ||
      isOnnxBuilderPattern(destination) ||
      hasOnnxNativeReference(from) ||
      hasOnnxNativeReference(to) ||
      hasOnnxNativeReference(filter)
    );
  });
  const fileSetLocationMatches =
    (allowedFileSetLocation === 'implicit-root' && from === '' && to === '') ||
    (allowedFileSetLocation === 'explicit-root' && from === '.' && to === '.');
  if (fileSetMentionsOnnx && !fileSetLocationMatches) {
    throw new Error(`Unexpected electron-builder ONNX FileSet in ${field}`);
  }
  collectOnnxBuilderPatterns(value.filter, field, found, allowedFileSetLocation);
}

const onnxBuilderPatternCache = new Map();

function isOnnxBuilderPattern(value) {
  if (value === '') return false;
  const cached = onnxBuilderPatternCache.get(value);
  if (cached !== undefined) return cached;
  const original = value.replaceAll('\\', '/').toLowerCase();
  if (original === '!node_modules/**/*') return false;
  const normalized = posix.normalize(original.replace(/^!/u, ''));
  const expandedPatterns = [
    normalized,
    normalized.replaceAll('${arch}', 'x64'),
    normalized.replaceAll('${arch}', 'arm64'),
    normalized.replace(/\$\{[^}]+\}/gu, '**'),
  ];
  const matches =
    hasOnnxNativeReference(normalized) ||
    ONNX_BUILDER_NATIVE_INVENTORY.some((entry) =>
      expandedPatterns.some((pattern) => minimatch(entry.toLowerCase(), pattern, { dot: true })),
    );
  onnxBuilderPatternCache.set(value, matches);
  return matches;
}

function hasOnnxNativeReference(value) {
  const normalized = value.toLowerCase();
  if (normalized.includes('napi-v3')) return true;
  if (!normalized.includes('onnxruntime')) return false;
  return ![
    'node_modules/onnxruntime-node/package.json',
    'node_modules/onnxruntime-node/dist/',
    'node_modules/onnxruntime-common/package.json',
    'node_modules/onnxruntime-common/dist/',
  ].some((allowed) => normalized === allowed || normalized.startsWith(allowed));
}

const OUT_EXACT_PATHS = new Set([
  'out',
  'out/main',
  'out/main/index.js',
  'out/main/chunks',
  'out/workers',
  'out/workers/whisper-bootstrap.cjs',
  'out/workers/whisper-payload.cjs',
  'out/preload',
  'out/preload/main.js',
  'out/preload/widget.js',
  'out/preload/capture.js',
  'out/renderer',
  'out/renderer/assets',
  'out/renderer/main',
  'out/renderer/main/index.html',
  'out/renderer/widget',
  'out/renderer/widget/index.html',
  'out/renderer/capture',
  'out/renderer/capture/index.html',
]);
export const PROVIDER_LOGO_BASENAMES = Object.freeze([
  'anthropic',
  'apipie',
  'azure',
  'bedrock',
  'cerebras',
  'cohere',
  'cometapi',
  'deepseek',
  'docker-model-runner',
  'fireworksai',
  'foundry-local',
  'gemini',
  'generic-openai',
  'giteeai',
  'groq',
  'koboldcpp',
  'lemonade',
  'litellm',
  'lmstudio',
  'localai',
  'minimax',
  'mistral',
  'moonshotai',
  'novita',
  'nvidia-nim',
  'ollama',
  'omlx',
  'openai',
  'openrouter',
  'perplexity',
  'pi',
  'ppio',
  'privatemode',
  'sambanova',
  'text-generation-webui',
  'togetherai',
  'xai',
  'zai',
]);
const JPEG_PROVIDER_LOGOS = new Set(['fireworksai', 'localai', 'mistral', 'openrouter']);
const PROVIDER_LOGO_PATTERN = new RegExp(
  `^out/renderer/assets/(${PROVIDER_LOGO_BASENAMES.join('|')})-[A-Za-z0-9_-]+\\.(png|jpeg)$`,
);
const REQUIRED_RENDERER_CHUNKS = Object.freeze([
  ['main', 'js'],
  ['main', 'css'],
  ['widget', 'js'],
  ['widget', 'css'],
  ['capture', 'js'],
  ['capture.worklet', 'js'],
  ['audio', 'js'],
  ['Dialog', 'js'],
  ['HistoryScreen', 'js'],
  ['InfoScreen', 'js'],
  ['SettingsScreen', 'js'],
  ['SmartProcessingSection', 'js'],
  ['UpdateDialog', 'js'],
  ['schemas', 'js'],
  ['theme', 'js'],
  ['theme', 'css'],
]);
const REQUIRED_BRAND_LOGOS = Object.freeze(['logo-light', 'logo-dark']);

function rendererChunkPattern(stem, extension) {
  return new RegExp(
    `^out/renderer/assets/${stem.replaceAll('.', '\\.')}-[A-Za-z0-9_-]+\\.${extension}$`,
  );
}

function isAllowedRendererAsset(entry) {
  if (
    REQUIRED_RENDERER_CHUNKS.some(([stem, extension]) =>
      rendererChunkPattern(stem, extension).test(entry),
    )
  ) {
    return true;
  }
  if (/^out\/renderer\/assets\/logo-(?:light|dark)-[A-Za-z0-9_-]+\.png$/u.test(entry)) {
    return true;
  }
  const provider = PROVIDER_LOGO_PATTERN.exec(entry);
  return (
    provider !== null && provider[2] === (JPEG_PROVIDER_LOGOS.has(provider[1]) ? 'jpeg' : 'png')
  );
}

function requireExactlyOneAsset(entries, pattern, label) {
  const count = entries.filter((entry) => pattern.test(entry)).length;
  if (count === 0) throw new Error(`Required renderer asset is missing: ${label}`);
  if (count !== 1) throw new Error(`Required renderer asset count is not one (${label}): ${count}`);
}
const ONNX_RESOURCE_PREFIX = 'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3';
const ONNX_RESOURCE_PATHS = Object.freeze({
  win: Object.freeze([
    `${ONNX_RESOURCE_PREFIX}/win32`,
    ...['x64', 'arm64'].flatMap((arch) => [
      `${ONNX_RESOURCE_PREFIX}/win32/${arch}`,
      `${ONNX_RESOURCE_PREFIX}/win32/${arch}/DirectML.dll`,
      `${ONNX_RESOURCE_PREFIX}/win32/${arch}/onnxruntime.dll`,
      `${ONNX_RESOURCE_PREFIX}/win32/${arch}/onnxruntime_binding.node`,
    ]),
  ]),
  mac: Object.freeze([
    `${ONNX_RESOURCE_PREFIX}/darwin`,
    ...['x64', 'arm64'].flatMap((arch) => [
      `${ONNX_RESOURCE_PREFIX}/darwin/${arch}`,
      `${ONNX_RESOURCE_PREFIX}/darwin/${arch}/libonnxruntime.1.21.0.dylib`,
      `${ONNX_RESOURCE_PREFIX}/darwin/${arch}/onnxruntime_binding.node`,
    ]),
  ]),
});
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

export function validateAsarEntries(entries, target) {
  const normalized = entries.map(normalizePackagePath);
  const allowedOnnxRuntimePaths = new Set(targetOnnxRuntimePaths(target));
  const unexpected = normalized.filter(
    (entry) =>
      entry.length > 0 &&
      !ASAR_EXACT_FILES.has(entry) &&
      !OUT_EXACT_PATHS.has(entry) &&
      !isAllowedRendererAsset(entry) &&
      !allowedOnnxRuntimePaths.has(entry),
  );
  assertSafePaths(normalized);
  if (unexpected.length > 0) {
    throw new Error(`Unexpected ASAR entries: ${unexpected.join(', ')}`);
  }
  for (const required of [
    'out/main/index.js',
    'out/workers/whisper-bootstrap.cjs',
    'out/workers/whisper-payload.cjs',
    'out/preload/main.js',
    'out/preload/widget.js',
    'out/preload/capture.js',
    'out/renderer/main/index.html',
    'out/renderer/widget/index.html',
    'out/renderer/capture/index.html',
    'package.json',
    'node_modules/better-sqlite3/lib/index.js',
    'node_modules/better-sqlite3/build/Release/better_sqlite3.node',
    'node_modules/bindings/bindings.js',
    'node_modules/file-uri-to-path/index.js',
    'node_modules/onnxruntime-node/dist/index.js',
    'node_modules/onnxruntime-common/dist/cjs/index.js',
  ]) {
    if (!normalized.includes(required))
      throw new Error(`Required runtime file is missing: ${required}`);
  }
  for (const required of requiredOnnxRuntimePaths(target)) {
    if (!normalized.includes(required)) {
      throw new Error(`Required ONNX runtime path is missing: ${required}`);
    }
  }
  for (const logo of PROVIDER_LOGO_BASENAMES) {
    const extension = JPEG_PROVIDER_LOGOS.has(logo) ? 'jpeg' : 'png';
    const asset = new RegExp(`^out/renderer/assets/${logo}-[A-Za-z0-9_-]+\\.${extension}$`);
    const count = normalized.filter((entry) => asset.test(entry)).length;
    if (count === 0) throw new Error(`Required provider logo is missing: ${logo}`);
    if (count !== 1) throw new Error(`Required provider logo count is not one (${logo}): ${count}`);
  }
  for (const [stem, extension] of REQUIRED_RENDERER_CHUNKS) {
    requireExactlyOneAsset(
      normalized,
      rendererChunkPattern(stem, extension),
      `${stem}.${extension}`,
    );
  }
  for (const logo of REQUIRED_BRAND_LOGOS) {
    requireExactlyOneAsset(
      normalized,
      new RegExp(`^out/renderer/assets/${logo}-[A-Za-z0-9_-]+\\.png$`),
      `${logo}.png`,
    );
  }
}

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

export function discoverFinalArtifactNames(fileNames) {
  return fileNames.filter((name) => /\.(?:exe|dmg|zip)$/iu.test(name));
}

export function validateSharedReleaseArtifacts(artifactNames, mode, expectedArtifact) {
  if (expectedArtifact.platform !== 'mac') {
    validateExpectedFinalArtifacts(artifactNames, mode, expectedArtifact);
    return;
  }
  for (const arch of ['x64', 'arm64']) {
    const marker = `-mac-${arch}.`;
    const matching = artifactNames.filter((name) => name.includes(marker));
    if (arch === expectedArtifact.arch || matching.length > 0)
      validateExpectedFinalArtifacts(matching, mode, { ...expectedArtifact, arch });
  }
  const unrelated = artifactNames.filter((name) => !/-mac-(?:x64|arm64)\.(?:dmg|zip)$/u.test(name));
  if (unrelated.length > 0)
    throw new Error(`Unexpected shared release artifacts: ${unrelated.join(', ')}`);
}

export function finalArtifactNamesForIdentity(artifactNames, expectedArtifact) {
  validateExpectedArtifactIdentity(expectedArtifact);
  const expectedStem = `Talking-Quill-${expectedArtifact.version}-${expectedArtifact.platform}-${expectedArtifact.arch}`;
  const expectedNamePattern = new RegExp(`^${escapeRegExp(expectedStem)}\\.(?:exe|dmg|zip)$`, 'u');
  return artifactNames.map(normalizePackagePath).filter((name) => expectedNamePattern.test(name));
}

export function validateExpectedFinalArtifacts(artifactNames, mode, expectedArtifact) {
  validateExpectedArtifactIdentity(expectedArtifact);
  const normalized = artifactNames.map(normalizePackagePath);
  const matching = finalArtifactNamesForIdentity(normalized, expectedArtifact);
  const unexpectedNames = normalized.filter((name) => !matching.includes(name));
  if (unexpectedNames.length > 0) {
    throw new Error(`Unexpected final artifact names: ${unexpectedNames.join(', ')}`);
  }
  const counts = {
    exe: normalized.filter((name) => /\.exe$/iu.test(name)).length,
    dmg: normalized.filter((name) => /\.dmg$/iu.test(name)).length,
    zip: normalized.filter((name) => /\.zip$/iu.test(name)).length,
  };
  const expected = {
    none: { exe: 0, dmg: 0, zip: 0 },
    'native-setup': { exe: 1, dmg: 0, zip: 0 },
    'dmg-zip': { exe: 0, dmg: 1, zip: 1 },
  }[mode];
  if (expected === undefined) {
    throw new Error(`Unknown final-artifact requirement mode: ${String(mode)}`);
  }
  if (
    normalized.length !== expected.exe + expected.dmg + expected.zip ||
    counts.exe !== expected.exe ||
    counts.dmg !== expected.dmg ||
    counts.zip !== expected.zip
  ) {
    throw new Error(
      `Final artifacts do not match ${mode}: expected exe=${String(expected.exe)}, dmg=${String(expected.dmg)}, zip=${String(expected.zip)}; found exe=${String(counts.exe)}, dmg=${String(counts.dmg)}, zip=${String(counts.zip)}`,
    );
  }
}

export function validateFinalArtifactInspection(produced, inspected, strict) {
  if (
    !Number.isInteger(produced) ||
    !Number.isInteger(inspected) ||
    produced < 0 ||
    inspected < 0 ||
    inspected > produced
  ) {
    throw new Error('Final-artifact inspection counts are invalid');
  }
  if (strict && inspected !== produced) {
    throw new Error(
      `Strict final-artifact inspection requires every produced artifact (${String(inspected)}/${String(produced)} inspected)`,
    );
  }
}

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

export function normalizePackagePath(path) {
  return path.replaceAll('\\', '/').replace(/^\/+/, '').replace(/\/$/, '');
}

const ATTRIBUTION_FILES = new Set(['LICENSE', 'THIRD_PARTY_NOTICES.txt']);

export function validateSecretContent(path, source) {
  const normalized = normalizePackagePath(path);
  const secretRules = findSecretRuleIds(source);
  if (secretRules.length > 0) {
    throw new Error(`Secret-like content (${secretRules.join(', ')}) is forbidden: ${normalized}`);
  }
}

export function validateRuntimeContent(path, source) {
  const normalized = normalizePackagePath(path);
  const basename = normalized.split('/').at(-1) ?? normalized;
  if (!ATTRIBUTION_FILES.has(basename) && /anything(?:[\s-])*llm/iu.test(source)) {
    throw new Error(`AnythingLLM runtime content is forbidden: ${normalized}`);
  }
  validateSecretContent(normalized, source);
}

function validateExpectedArtifactIdentity(expectedArtifact) {
  if (
    expectedArtifact === null ||
    typeof expectedArtifact !== 'object' ||
    !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/u.test(expectedArtifact.version) ||
    !['win', 'mac'].includes(expectedArtifact.platform) ||
    !['x64', 'arm64'].includes(expectedArtifact.arch)
  ) {
    throw new Error('Expected final-artifact version, platform, and architecture are invalid');
  }
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
}

function assertSafePaths(entries) {
  const forbidden = entries.filter((entry) => {
    const lower = entry.toLowerCase();
    if (FORBIDDEN_PARTS.some((part) => lower.includes(part))) {
      return true;
    }
    const dot = lower.lastIndexOf('.');
    return dot >= 0 && FORBIDDEN_EXTENSIONS.has(lower.slice(dot));
  });
  if (forbidden.length > 0) {
    throw new Error(`Forbidden packaged files: ${forbidden.join(', ')}`);
  }
}
