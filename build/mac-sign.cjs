'use strict';

const path = require('node:path');

const RUST_HELPER_RELATIVE_PATH = 'Contents/Resources/helper/talking-quill-helper';

function isRustHelper(appPath, filePath) {
  const relative = path
    .relative(path.resolve(appPath), path.resolve(filePath))
    .split(path.sep)
    .join('/');
  return relative === RUST_HELPER_RELATIVE_PATH;
}

function optionsForSignedFile(configuration, filePath) {
  const inherited = configuration.optionsForFile?.(filePath) ?? {};
  if (!isRustHelper(configuration.app, filePath)) return inherited;
  return { ...inherited, entitlements: [], hardenedRuntime: true };
}

async function signWithLeastPrivilege(configuration) {
  const { signAsync } = require('@electron/osx-sign');
  await signAsync({
    ...configuration,
    optionsForFile: (filePath) => optionsForSignedFile(configuration, filePath),
  });
}

module.exports = signWithLeastPrivilege;
module.exports.isRustHelper = isRustHelper;
module.exports.optionsForSignedFile = optionsForSignedFile;
module.exports.RUST_HELPER_RELATIVE_PATH = RUST_HELPER_RELATIVE_PATH;
