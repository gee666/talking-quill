import { assertSafePaths, normalizePackagePath } from './package-path-policy.mjs';
import { targetOnnxRuntimePaths, requiredOnnxRuntimePaths } from './package-onnx-policy.mjs';

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
