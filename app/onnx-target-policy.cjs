const ONNX_NATIVE_PATTERN = 'node_modules/onnxruntime-node/bin/napi-v3/**/*';
const ONNX_NATIVE_ROOT = 'node_modules/onnxruntime-node/bin/napi-v3';
const ONNX_NATIVE_PREFIX = `${ONNX_NATIVE_ROOT}/`;

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
  const files = replaceNativePattern(config.files, targetPattern, 'files', true);
  const asarUnpack = replaceNativePattern(config.asarUnpack, targetPattern, 'asarUnpack', false);
  config.files = files;
  config.asarUnpack = asarUnpack;
  return targetPattern;
}

function replaceNativePattern(value, targetPattern, field, allowFileSets) {
  const state = { selectors: 0 };
  const replaceString = (pattern) => {
    if (pattern === ONNX_NATIVE_PATTERN) {
      state.selectors += 1;
      return targetPattern;
    }
    if (pattern.includes(ONNX_NATIVE_ROOT)) {
      throw new Error(`electron-builder ${field} contains an unsupported ONNX native selector`);
    }
    return pattern;
  };

  const replaceFileSet = (fileSet) => {
    if (!allowFileSets || fileSet === null || Array.isArray(fileSet)) {
      throw new Error(`electron-builder ${field} contains an unsupported file set`);
    }
    if (typeof fileSet !== 'object') {
      throw new Error(`electron-builder ${field} contains an unsupported value`);
    }
    if (
      (typeof fileSet.from === 'string' && fileSet.from.includes(ONNX_NATIVE_ROOT)) ||
      (typeof fileSet.to === 'string' && fileSet.to.includes(ONNX_NATIVE_ROOT))
    ) {
      throw new Error(`electron-builder ${field} contains an unsupported ONNX file set`);
    }
    if (fileSet.filter === undefined) return fileSet;
    if (fileSet.from !== undefined || fileSet.to !== undefined) {
      const filters = Array.isArray(fileSet.filter) ? fileSet.filter : [fileSet.filter];
      if (
        filters.some((pattern) => typeof pattern === 'string' && pattern.includes(ONNX_NATIVE_ROOT))
      ) {
        throw new Error(`electron-builder ${field} contains an ambiguous ONNX file set`);
      }
      return fileSet;
    }
    return { ...fileSet, filter: replaceValue(fileSet.filter, replaceString, field, false) };
  };

  const result = replaceValue(value, replaceString, field, allowFileSets, replaceFileSet);
  if (state.selectors === 0) {
    throw new Error(`electron-builder ${field} is missing its ONNX native selector`);
  }
  if (state.selectors !== 1) {
    throw new Error(`electron-builder ${field} must contain exactly one ONNX native selector`);
  }
  return result;
}

function replaceValue(value, replaceString, field, allowObjects, replaceObject) {
  if (typeof value === 'string') return replaceString(value);
  if (Array.isArray(value)) {
    return value.map((item) => {
      if (typeof item === 'string') return replaceString(item);
      if (allowObjects) return replaceObject(item);
      throw new Error(`electron-builder ${field} contains an unsupported value`);
    });
  }
  if (allowObjects) return replaceObject(value);
  throw new Error(`electron-builder ${field} must be a string or an array of strings`);
}

module.exports = {
  ONNX_NATIVE_PATTERN,
  applyTargetNativeOnnxPolicy,
  electronBuilderTarget,
  targetNativeOnnxPattern,
};
