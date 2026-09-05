import { findSecretRuleIds } from './secret-rules.mjs';
import { normalizePackagePath } from './package-path-policy.mjs';

export { normalizePackagePath } from './package-path-policy.mjs';
export { validateAsarEntries, PROVIDER_LOGO_BASENAMES } from './package-asar-policy.mjs';
export {
  ONNX_RUNTIME_PATHS,
  ONNX_BUILDER_NATIVE_INVENTORY,
  validateElectronBuilderOnnxConfig,
} from './package-onnx-policy.mjs';
export {
  validatePhysicalEntries,
  validatePhysicalPackageEntries,
  validateResourceEntries,
} from './package-physical-policy.mjs';

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
  const expectedStem = `Talking-Quill-${expectedArtifact.version}-${expectedArtifact.platform}-${expectedArtifact.arch}${expectedArtifact.artifactKind === undefined ? '' : `-${expectedArtifact.artifactKind}`}`;
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
    !['x64', 'arm64'].includes(expectedArtifact.arch) ||
    (expectedArtifact.artifactKind !== undefined &&
      (expectedArtifact.platform !== 'win' ||
        !/^[A-Za-z0-9]+(?:-[A-Za-z0-9]+)*$/u.test(expectedArtifact.artifactKind)))
  ) {
    throw new Error('Expected final-artifact version, platform, and architecture are invalid');
  }
}

function escapeRegExp(value) {
  return value.replace(/[.*+?^${}()|[\]\\]/gu, '\\$&');
}
