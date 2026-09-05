import { createRequire } from 'node:module';
import { posix } from 'node:path';

const require = createRequire(import.meta.url);
const electronBuilderRequire = createRequire(require.resolve('electron-builder/package.json'));
const { minimatch } = electronBuilderRequire('minimatch');

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

export function targetOnnxRuntimePaths(target) {
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

export function requiredOnnxRuntimePaths(target) {
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
