const PRESERVED_ENVIRONMENT_NAMES = new Set([
  'APPDATA',
  'COMSPEC',
  'HOME',
  'HOMEDRIVE',
  'HOMEPATH',
  'LANG',
  'LANGUAGE',
  'LC_ALL',
  'LC_CTYPE',
  'LOCALAPPDATA',
  'LOGNAME',
  'PATH',
  'PATHEXT',
  'PROGRAMDATA',
  'PROGRAMFILES',
  'PROGRAMFILES(X86)',
  'PROGRAMW6432',
  'PSMODULEPATH',
  'SYSTEMDRIVE',
  'SYSTEMROOT',
  'TEMP',
  'TMP',
  'TMPDIR',
  'TZ',
  'USER',
  'USERNAME',
  'USERPROFILE',
  'WINDIR',
]);

const TOOLCHAIN_ENVIRONMENT_NAMES = new Set([
  'AR',
  'CARGO_HOME',
  'CARGO_TARGET_DIR',
  'CC',
  'CXX',
  'DEVELOPER_DIR',
  'INCLUDE',
  'LIB',
  'LIBPATH',
  'MACOSX_DEPLOYMENT_TARGET',
  'PKG_CONFIG_PATH',
  'RUSTC',
  'RUSTDOC',
  'RUSTUP_HOME',
  'RUSTUP_TOOLCHAIN',
  'SDKROOT',
]);

const TOOLCHAIN_ENVIRONMENT_PATTERNS = [
  /^(?:AR|CC|CXX)_[A-Z0-9_]+$/u,
  /^CARGO_TARGET_[A-Z0-9_]+_(?:AR|LINKER|RUNNER)$/u,
  /^(?:VC|VS)[A-Z0-9_]*$/u,
  /^VSCMD_[A-Z0-9_]+$/u,
  /^WINDOWSSDK[A-Z0-9_]*$/u,
  /^UCRT[A-Z0-9_]*$/u,
];

const FORBIDDEN_TALKING_QUILL_ENVIRONMENT =
  /^TALKING_QUILL_.*(?:PACKAGE|PREDECESSOR|FAULT|ACCEPTANCE|PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u;

export function normalizeEnvironment(environment) {
  const entries = new Map();
  for (const [name, value] of Object.entries(environment)) {
    const normalizedName = name.toUpperCase();
    entries.set(normalizedName, [
      normalizedName.startsWith('TALKING_QUILL_') ? normalizedName : name,
      value,
    ]);
  }
  return Object.fromEntries(entries.values());
}

export function sanitizedSubprocessEnvironment(source = process.env, overrides = {}) {
  const normalizedSource = normalizeEnvironment(source);
  const environment = {};
  for (const [name, value] of Object.entries(normalizedSource)) {
    const normalizedName = name.toUpperCase();
    if (FORBIDDEN_TALKING_QUILL_ENVIRONMENT.test(normalizedName)) continue;
    if (
      PRESERVED_ENVIRONMENT_NAMES.has(normalizedName) ||
      TOOLCHAIN_ENVIRONMENT_NAMES.has(normalizedName) ||
      TOOLCHAIN_ENVIRONMENT_PATTERNS.some((pattern) => pattern.test(normalizedName))
    ) {
      environment[name] = value;
    }
  }
  const normalizedOverrides = normalizeEnvironment(overrides);
  for (const [name, value] of Object.entries(normalizedOverrides)) environment[name] = value;
  return normalizeEnvironment(environment);
}
