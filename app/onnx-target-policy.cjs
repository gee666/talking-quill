const ONNX_NATIVE_PATTERN = 'node_modules/onnxruntime-node/bin/napi-v3/**/*';
const ONNX_NATIVE_PREFIX = 'node_modules/onnxruntime-node/bin/napi-v3/';

function electronBuilderTarget(context) {
  const platform = context?.electronPlatformName;
  const architecture = context?.arch === 1 ? 'x64' : context?.arch === 3 ? 'arm64' : null;
  if (!['win32', 'darwin'].includes(platform) || architecture === null) {
    throw new Error(
      `Unsupported ONNX package target: ${String(platform)}/${String(context?.arch)}`,
    );
  }
  return { platform, architecture };
}

function targetNativeOnnxPattern(target) {
  if (
    !['win32', 'darwin'].includes(target?.platform) ||
    !['x64', 'arm64'].includes(target?.architecture)
  ) {
    throw new Error('ONNX target must specify win32|darwin and x64|arm64');
  }
  return `${ONNX_NATIVE_PREFIX}${target.platform}/${target.architecture}/**/*`;
}

function applyTargetNativeOnnxPolicy(config, target) {
  const targetPattern = targetNativeOnnxPattern(target);
  config.files = replaceNativePattern(config.files, targetPattern, 'files');
  config.asarUnpack = replaceNativePattern(config.asarUnpack, targetPattern, 'asarUnpack');
  return targetPattern;
}

function replaceNativePattern(patterns, targetPattern, field) {
  if (!Array.isArray(patterns)) throw new Error(`electron-builder ${field} must be an array`);
  let insertionIndex = -1;
  const retained = [];
  for (const pattern of patterns) {
    if (typeof pattern !== 'string') {
      if (JSON.stringify(pattern).includes(ONNX_NATIVE_PREFIX)) {
        throw new Error(`electron-builder ${field} contains an unsupported ONNX file set`);
      }
      retained.push(pattern);
      continue;
    }
    if (pattern.startsWith(`!${ONNX_NATIVE_PREFIX}`)) {
      throw new Error(`electron-builder ${field} contains an ONNX exclusion`);
    }
    if (pattern === ONNX_NATIVE_PATTERN || pattern.startsWith(ONNX_NATIVE_PREFIX)) {
      if (insertionIndex < 0) insertionIndex = retained.length;
      continue;
    }
    retained.push(pattern);
  }
  if (insertionIndex < 0) {
    throw new Error(`electron-builder ${field} is missing its ONNX native selector`);
  }
  retained.splice(insertionIndex, 0, targetPattern);
  return retained;
}

module.exports = {
  ONNX_NATIVE_PATTERN,
  applyTargetNativeOnnxPolicy,
  electronBuilderTarget,
  targetNativeOnnxPattern,
};
