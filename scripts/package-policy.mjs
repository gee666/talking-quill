import { findSecretRuleIds } from './secret-rules.mjs';

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
const ONNX_RUNTIME_PATTERN =
  /^node_modules\/onnxruntime-(?:node\/(?:dist\/[\w-]+\.js|bin\/napi-v3(?:\/(?:darwin|win32|linux)(?:\/(?:x64|arm64)(?:\/[\w.-]+)?)?)?)|common\/dist\/cjs\/[\w-]+\.js)$/;
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
const PROVIDER_LOGO_PATTERN = new RegExp(
  `^out/renderer/assets/(?:${PROVIDER_LOGO_BASENAMES.join('|')})-[A-Za-z0-9_-]+\\.(?:png|jpeg)$`,
);
const OUT_ASSET_PATTERN =
  /^out\/renderer\/assets\/(?:(?:main|widget|capture|capture\.worklet|audio|status-presentation|Status)-[A-Za-z0-9_-]+\.(?:js|css)|app-icon-[A-Za-z0-9_-]+\.png)$/;
const COMMON_RESOURCE_PATHS = [
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
  'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3',
  'helper',
];
const ONNX_RESOURCE_PATTERN =
  /^app\.asar\.unpacked\/node_modules\/onnxruntime-node\/bin\/napi-v3\/(?:darwin|win32|linux)(?:\/(?:x64|arm64)(?:\/[\w.-]+)?)?$/;
const PLATFORM_RESOURCE_PATHS = Object.freeze({
  win: Object.freeze(['elevate.exe', 'helper/talking-quill-helper.exe']),
  mac: Object.freeze(['electron.icns', 'helper/talking-quill-helper']),
});

export function validateAsarEntries(entries) {
  const normalized = entries.map(normalizePackagePath);
  const unexpected = normalized.filter(
    (entry) =>
      entry.length > 0 &&
      !ASAR_EXACT_FILES.has(entry) &&
      !OUT_EXACT_PATHS.has(entry) &&
      !OUT_ASSET_PATTERN.test(entry) &&
      !ONNX_RUNTIME_PATTERN.test(entry) &&
      !PROVIDER_LOGO_PATTERN.test(entry),
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
  for (const logo of PROVIDER_LOGO_BASENAMES) {
    const asset = new RegExp(`^out/renderer/assets/${logo}-[A-Za-z0-9_-]+\\.(?:png|jpeg)$`);
    if (!normalized.some((entry) => asset.test(entry))) {
      throw new Error(`Required provider logo is missing: ${logo}`);
    }
  }
  for (const asset of [
    /^out\/renderer\/assets\/main-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/main-[A-Za-z0-9_-]+\.css$/,
    /^out\/renderer\/assets\/widget-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/widget-[A-Za-z0-9_-]+\.css$/,
    /^out\/renderer\/assets\/capture-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/capture\.worklet-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/audio-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/(?:status-presentation|Status)-[A-Za-z0-9_-]+\.js$/,
    /^out\/renderer\/assets\/(?:status-presentation|Status)-[A-Za-z0-9_-]+\.css$/,
  ]) {
    if (!normalized.some((entry) => asset.test(entry))) {
      throw new Error(`Required renderer asset is missing: ${String(asset)}`);
    }
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
const MAC_FRAMEWORK_PATTERN =
  /^Talking Quill\.app\/Contents\/Frameworks\/(?:Electron Framework\.framework|Mantle\.framework|ReactiveObjC\.framework|Squirrel\.framework|Sparkle\.framework|Talking Quill Helper(?: \(GPU\)| \(Plugin\)| \(Renderer\))?\.app)(?:\/.*)?$/u;
const MAC_PHYSICAL_EXACT = new Set([
  '.background',
  '.background/background.tiff',
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

export function validateExpectedFinalArtifacts(artifactNames, mode, expectedArtifact) {
  const normalized = artifactNames.map(normalizePackagePath);
  if (
    expectedArtifact === null ||
    typeof expectedArtifact !== 'object' ||
    !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/u.test(expectedArtifact.version) ||
    !['win', 'mac'].includes(expectedArtifact.platform) ||
    !['x64', 'arm64'].includes(expectedArtifact.arch)
  ) {
    throw new Error('Expected final-artifact version, platform, and architecture are invalid');
  }
  const expectedStem = `Talking-Quill-${expectedArtifact.version}-${expectedArtifact.platform}-${expectedArtifact.arch}`;
  const expectedNamePattern = new RegExp(`^${escapeRegExp(expectedStem)}\\.(?:exe|dmg|zip)$`, 'u');
  const unexpectedNames = normalized.filter((name) => !expectedNamePattern.test(name));
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
    nsis: { exe: 1, dmg: 0, zip: 0 },
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

export function validatePhysicalPackageEntries(entries, target) {
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
    return (
      !MAC_PHYSICAL_EXACT.has(entry) &&
      !entry.startsWith('Talking Quill.app/Contents/Resources/') &&
      !MAC_FRAMEWORK_PATTERN.test(entry)
    );
  });
  if (unexpected.length > 0) {
    throw new Error(`Unexpected physical package entries: ${unexpected.join(', ')}`);
  }
}

export function validateResourceEntries(entries, target) {
  const normalized = entries.map(normalizePackagePath);
  const allowed = new Set([...COMMON_RESOURCE_PATHS, ...PLATFORM_RESOURCE_PATHS[target]]);
  assertSafePaths(normalized);
  const unexpected = normalized.filter(
    (entry) => entry.length > 0 && !allowed.has(entry) && !ONNX_RESOURCE_PATTERN.test(entry),
  );
  if (unexpected.length > 0) {
    throw new Error(`Unexpected packaged resources: ${unexpected.join(', ')}`);
  }
  const helper =
    target === 'mac' ? 'helper/talking-quill-helper' : 'helper/talking-quill-helper.exe';
  for (const required of [
    'app.asar',
    'LICENSE',
    'THIRD_PARTY_NOTICES.txt',
    'app.asar.unpacked/node_modules/better-sqlite3/build/Release/better_sqlite3.node',
    'app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3',
    helper,
  ]) {
    if (!normalized.includes(required)) {
      throw new Error(`Required packaged resource is missing: ${required}`);
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
