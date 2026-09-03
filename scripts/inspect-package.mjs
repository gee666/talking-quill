import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { zstdDecompressSync } from 'node:zlib';
import { createReadStream, existsSync } from 'node:fs';
import {
  appendFile,
  lstat,
  readFile,
  readdir,
  readlink,
  realpath,
  rm,
  mkdir,
  writeFile,
} from 'node:fs/promises';
import { basename, dirname, relative, resolve, sep } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { extractFile, listPackage, statFile } from '@electron/asar';
import { extractRegularAsarFiles } from './asar-entry-inspection.mjs';
import { bindTqpkg2OwnerManifest, parseTqpkg2 } from './tqpkg2.mjs';
import { FuseV1Options, getCurrentFuseWire } from '@electron/fuses';
import {
  discoverFinalArtifactNames,
  finalArtifactNamesForIdentity,
  normalizePackagePath,
  validateAsarEntries,
  validateSharedReleaseArtifacts,
  validateFinalArtifactInspection,
  validatePhysicalEntries,
  validatePhysicalPackageEntries,
  validateResourceEntries,
  validateRuntimeContent,
  validateSecretContent,
} from './package-policy.mjs';
import { SECRET_SCAN_OVERLAP_BYTES } from './secret-rules.mjs';
import {
  verifyCompleteNativeRoleInventory,
  verifyHelperBuildContract,
  verifyMacosServiceBridgeBuildContract,
  verifyNativeSourceIdentity,
  verifyOwnerBuildContract,
  verifyWindowsUpdateRecoveryLauncherBuildContract,
} from './helper-build-contract.mjs';
import { inspectNativeTree, readNativeArchitectures } from './native-architecture.mjs';
import {
  artifactUploadPaths,
  verifyArtifactProvenanceManifest,
  writeArtifactProvenanceManifest,
} from './artifact-provenance.mjs';
import {
  RELEASE_PACKAGE_METADATA_NAME,
  validatePackageReleaseMetadata,
  verifyMatchingPackageReleaseMetadataBytes,
  verifySerializedPackageReleaseMetadata,
} from './release-package-metadata.mjs';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { currentSourceIdentity } from './source-identity.mjs';

const require = createRequire(import.meta.url);
const {
  FORBIDDEN_MARKER_OVERLAP_BYTES,
  assertNoForbiddenProductionMarkers,
} = require('./forbidden-production-markers.cjs');
const invocationDirectory = process.cwd();
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const subprocessEnvironment = sanitizedSubprocessEnvironment();
const packageArgument = process.argv
  .slice(2)
  .find((argument) => argument !== '--' && !argument.startsWith('--'));
const macosOwnerPackage = process.argv.includes('--macos-owner');
const packageVariant = process.env.TALKING_QUILL_PACKAGE_VARIANT ?? 'canonical';
if (
  !['canonical', 'directory-test', 'installed-acceptance', 'packaged-test'].includes(packageVariant)
) {
  throw new Error(`Unknown package inspection variant: ${packageVariant}`);
}
const canonicalPackage = ['canonical', 'directory-test'].includes(packageVariant);
const windowsInstalledAcceptance = packageVariant === 'installed-acceptance';
const strictArtifactInspection =
  process.argv.includes('--strict') || process.env.TALKING_QUILL_PACKAGE_INSPECTION_STRICT === '1';
const artifactRequirementArgument = process.argv.find((argument) =>
  argument.startsWith('--artifacts-required='),
);
const artifactRequirement =
  artifactRequirementArgument?.slice('--artifacts-required='.length) ??
  process.env.TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED;
if (strictArtifactInspection && artifactRequirement === undefined) {
  throw new Error(
    'Strict final-artifact inspection requires --artifacts-required=none|native-setup|dmg-zip or TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED',
  );
}
const packageRoot = resolve(invocationDirectory, packageArgument ?? 'release/win-unpacked');
process.chdir(repositoryRoot);
const appManifest = JSON.parse(await readFile(resolve('app/package.json'), 'utf8'));
const expectedVersion = appManifest.version;
const sourceIdentity = currentSourceIdentity({ repositoryRoot });
if (typeof expectedVersion !== 'string' || !/^[0-9A-Za-z][0-9A-Za-z.+-]*$/u.test(expectedVersion)) {
  throw new Error('Application package version is invalid');
}
const expectedPlatform = process.env.TALKING_QUILL_PACKAGE_TARGET;
const expectedArch = process.env.TALKING_QUILL_PACKAGE_ARCH;
if (
  strictArtifactInspection &&
  (!['win', 'mac'].includes(expectedPlatform ?? '') ||
    !['x64', 'arm64'].includes(expectedArch ?? ''))
) {
  throw new Error(
    'Strict package inspection requires TALKING_QUILL_PACKAGE_TARGET=win|mac and TALKING_QUILL_PACKAGE_ARCH=x64|arm64',
  );
}
const macBundle = resolve(packageRoot, 'Talking Quill.app');
const isMacBundle = existsSync(macBundle);
const packagePlatform = isMacBundle ? 'mac' : 'win';
const boundPlatform = expectedPlatform ?? packagePlatform;
const boundArch = expectedArch ?? process.arch;
if (boundPlatform !== packagePlatform) {
  throw new Error(
    `Package root platform ${packagePlatform} does not match expected ${boundPlatform}`,
  );
}
const resources = isMacBundle
  ? resolve(macBundle, 'Contents', 'Resources')
  : resolve(packageRoot, 'resources');
const asarPath = resolve(resources, 'app.asar');
if (!existsSync(asarPath)) throw new Error(`Missing package: ${asarPath}`);

const asarEntries = listPackage(asarPath).map(normalizePackagePath);
validateAsarEntries(asarEntries, { platform: boundPlatform, architecture: boundArch });
const testHarnessMarkers = [
  'talking-quill:task6-test-driver',
  'activationDown',
  'setWelcomePrerequisites',
  'TALKING_QUILL_TASK6_TEST_HARNESS',
  'TALKING_QUILL_VOCABULARY_TEST_HARNESS',
  'task6-test-composition',
  'source-test-dialogs',
];
inspectAsarContent(asarPath, asarEntries, 'app.asar');
validatePackagedWhisper(asarPath);
const nativeEntries = [
  'node_modules/better-sqlite3/build/Release/better_sqlite3.node',
  `node_modules/onnxruntime-node/bin/napi-v3/${boundPlatform === 'mac' ? 'darwin' : 'win32'}/${boundArch}/onnxruntime_binding.node`,
];
for (const nativeEntry of nativeEntries) {
  const nativeMetadata = statFile(asarPath, nativeEntry.replaceAll('/', sep));
  if (!('unpacked' in nativeMetadata) || nativeMetadata.unpacked !== true) {
    throw new Error(`Native runtime is not marked unpacked: ${nativeEntry}`);
  }
}
const resourceEntries = await walkResources(resources);
validateResourceEntries(resourceEntries, isMacBundle ? 'mac' : 'win', {
  architecture: boundArch,
  macosOwner: macosOwnerPackage,
  windowsInstalledAcceptance,
});
const physicalEntries = await inspectPhysicalTree(packageRoot, isMacBundle);
validatePhysicalPackageEntries(physicalEntries, isMacBundle ? 'mac' : 'win', {
  macosOwner: macosOwnerPackage,
});
const unpackedNativeEntries = await inspectNativeTree(packageRoot, {
  platform: boundPlatform,
  architecture: boundArch,
  exceptions: nativeArchitectureExceptions(isMacBundle),
});
if (unpackedNativeEntries.length === 0) {
  throw new Error('No native executable images were discovered in the unpacked package');
}
const unpackedReleaseMetadata =
  !isMacBundle || macosOwnerPackage
    ? await readFile(resolve(resources, RELEASE_PACKAGE_METADATA_NAME))
    : null;
const unpackedReleaseIdentity =
  unpackedReleaseMetadata === null
    ? null
    : validatePackageReleaseMetadata(JSON.parse(unpackedReleaseMetadata.toString('utf8')));
const artifactEvidence = await inspectFinalArtifacts(
  packageRoot,
  isMacBundle,
  strictArtifactInspection,
  artifactRequirement,
  {
    version: expectedVersion,
    platform: boundPlatform,
    arch: boundArch,
    ...(boundPlatform === 'win'
      ? {
          packageMode: unpackedReleaseIdentity?.packageMode,
          artifactKind:
            process.env.TALKING_QUILL_NATIVE_FAULT_PHASE === undefined
              ? unpackedReleaseIdentity?.packageMode === 'fresh'
                ? 'setup'
                : unpackedReleaseIdentity?.packageMode
              : `repair-${process.env.TALKING_QUILL_NATIVE_FAULT_PHASE}`,
        }
      : {}),
  },
  unpackedReleaseMetadata,
);
const noticeCheck = spawnSync(process.execPath, ['scripts/generate-notices.mjs', '--check'], {
  stdio: 'inherit',
  env: subprocessEnvironment,
});
if (noticeCheck.status !== 0)
  throw new Error('Generated notice content failed current inventory validation.');
const [packagedNotices, generatedNotices] = await Promise.all([
  readFile(resolve(resources, 'THIRD_PARTY_NOTICES.txt'), 'utf8'),
  readFile(resolve('app/assets/THIRD_PARTY_NOTICES.txt'), 'utf8'),
]);
if (packagedNotices !== generatedNotices) {
  throw new Error('Packaged third-party notices do not match the deterministic generated file.');
}
for (const nativeEntry of nativeEntries) {
  const physicalNative = await lstat(resolve(resources, 'app.asar.unpacked', nativeEntry));
  if (!physicalNative.isFile() || physicalNative.size === 0) {
    throw new Error(`Native runtime is not a non-empty regular file: ${nativeEntry}`);
  }
}
const platformLibrary =
  boundPlatform === 'mac'
    ? `node_modules/onnxruntime-node/bin/napi-v3/darwin/${boundArch}/libonnxruntime.1.21.0.dylib`
    : `node_modules/onnxruntime-node/bin/napi-v3/win32/${boundArch}/onnxruntime.dll`;
const physicalLibrary = await lstat(resolve(resources, 'app.asar.unpacked', platformLibrary));
if (!physicalLibrary.isFile() || physicalLibrary.size === 0) {
  throw new Error(`ONNX shared library is missing: ${platformLibrary}`);
}
const helperName = isMacBundle ? 'talking-quill-helper' : 'talking-quill-helper.exe';
const helper = resolve(resources, 'helper', helperName);
const helperMetadata = await lstat(helper);
if (!helperMetadata.isFile() || helperMetadata.isSymbolicLink() || helperMetadata.size === 0) {
  throw new Error(`Native helper is not a non-empty regular file: helper/${helperName}`);
}
if (isMacBundle && (helperMetadata.mode & 0o111) === 0) {
  throw new Error('macOS native helper is not executable');
}
await verifyHelperBuildContract(helper, { windows: !isMacBundle });
await verifyNativeSourceIdentity(helper, sourceIdentity);
if (macosOwnerPackage) {
  if (!isMacBundle) throw new Error('The keyboard-owner package mode is macOS-only');
  const owner = resolve(
    macBundle,
    'Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
  );
  const ownerMetadata = await lstat(owner);
  if (
    !ownerMetadata.isFile() ||
    ownerMetadata.isSymbolicLink() ||
    ownerMetadata.size === 0 ||
    (ownerMetadata.mode & 0o111) === 0
  ) {
    throw new Error('Nested keyboard owner is not a non-empty executable regular file');
  }
  await verifyOwnerBuildContract(owner);
  await verifyNativeSourceIdentity(owner, sourceIdentity);
  const bridge = resolve(macBundle, 'Contents/MacOS/talking-quill-macos-service-bridge');
  await verifyMacosServiceBridgeBuildContract(bridge);
  await verifyNativeSourceIdentity(bridge, sourceIdentity);
  await verifyCompleteNativeRoleInventory(
    unpackedNativeEntries.map((entry) => resolve(packageRoot, entry.path)),
    [helper, owner, bridge],
  );
  const ownerBytes = await readFile(owner);
  for (const forbiddenFixtureMarker of [
    'macos-native-lifecycle-fixture',
    'TALKING_QUILL_MACOS_REMOVAL_RETRY_FIXTURE',
    'permissioned-ci-v1',
  ]) {
    if (ownerBytes.includes(Buffer.from(forbiddenFixtureMarker))) {
      throw new Error(`Packaged owner contains lifecycle test hook: ${forbiddenFixtureMarker}`);
    }
  }
  const installedMarker = resolve(resources, 'keyboard-owner-installed-v1');
  const installedMarkerMetadata = await lstat(installedMarker);
  if (
    !installedMarkerMetadata.isFile() ||
    installedMarkerMetadata.isSymbolicLink() ||
    (await readFile(installedMarker, 'utf8')) !== 'talking-quill-keyboard-owner-v1\n'
  ) {
    throw new Error('Installed keyboard-owner marker is missing or invalid');
  }
  const denialAddon = resolve(resources, 'macos-keychain-denial.node');
  const denialAddonMetadata = await lstat(denialAddon);
  if (
    !denialAddonMetadata.isFile() ||
    denialAddonMetadata.isSymbolicLink() ||
    denialAddonMetadata.size === 0 ||
    (denialAddonMetadata.mode & 0o111) === 0
  ) {
    throw new Error('macOS Keychain denial addon is missing or invalid');
  }
  // One outer sealed policy avoids a nested owner CodeDirectory/hash cycle.
  for (const policy of [resolve(resources, 'keyboard-owner-r5m.json')]) {
    const metadata = await lstat(policy);
    if (
      !metadata.isFile() ||
      metadata.isSymbolicLink() ||
      metadata.size === 0 ||
      metadata.size > 64 * 1024
    ) {
      throw new Error('Installed keyboard-owner policy is missing or invalid');
    }
  }
}
if (!isMacBundle) {
  const windowsRoleDirectory = resolve(resources, 'helper');
  const windowsOwner = resolve(windowsRoleDirectory, 'talking-quill-keyboard-owner.exe');
  const recoveryLauncher = resolve(
    windowsRoleDirectory,
    'talking-quill-update-recovery-launcher.exe',
  );
  await verifyOwnerBuildContract(windowsOwner);
  await verifyNativeSourceIdentity(windowsOwner, sourceIdentity);
  await verifyWindowsUpdateRecoveryLauncherBuildContract(recoveryLauncher);
  await verifyNativeSourceIdentity(recoveryLauncher, sourceIdentity);
  await verifyCompleteNativeRoleInventory(
    unpackedNativeEntries.map((entry) => resolve(packageRoot, entry.path)),
    [helper, windowsOwner, recoveryLauncher],
  );
}
if (!isMacBundle || macosOwnerPackage) {
  await verifySerializedPackageReleaseMetadata(
    resolve(resources, RELEASE_PACKAGE_METADATA_NAME),
    packageRoot,
    { version: expectedVersion, platform: boundPlatform, architecture: boundArch },
  );
}

const executable = isMacBundle
  ? resolve(macBundle, 'Contents', 'MacOS', 'Talking Quill')
  : resolve(packageRoot, 'Talking Quill.exe');
const wire = await getCurrentFuseWire(executable);
const expected = new Map([
  [FuseV1Options.RunAsNode, '0'],
  [FuseV1Options.EnableCookieEncryption, '1'],
  [FuseV1Options.EnableNodeOptionsEnvironmentVariable, '0'],
  [FuseV1Options.EnableNodeCliInspectArguments, '0'],
  [FuseV1Options.EnableEmbeddedAsarIntegrityValidation, '1'],
  [FuseV1Options.OnlyLoadAppFromAsar, '1'],
]);
for (const [fuse, state] of expected) {
  if (String.fromCharCode(wire[fuse]) !== state) {
    throw new Error(`Electron fuse ${fuse} is not ${state}`);
  }
}
await writeArtifactProvenanceManifest({
  version: expectedVersion,
  platform: boundPlatform,
  arch: boundArch,
  packageRoot,
  artifacts: artifactEvidence.artifacts,
});
const provenance = await verifyArtifactProvenanceManifest();
await emitArtifactPaths(artifactUploadPaths(provenance));
console.log(
  `Package allowlist, ${boundArch} target, ${unpackedNativeEntries.length} recursively identified native images, physical tree, ASAR/unpacked content, links, secrets, Electron fuses, and canonical provenance verified (${asarEntries.length} ASAR entries, ${resourceEntries.length} resources, ${physicalEntries.length} physical entries; final artifacts: ${artifactEvidence.summary})`,
);

async function walkResources(root) {
  const entries = [];
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const absolute = resolve(directory, entry.name);
      const metadata = await lstat(absolute);
      const name = normalizePackagePath(relative(root, absolute));
      if (metadata.isSymbolicLink())
        throw new Error(`Symlink is not allowed in resources: ${name}`);
      entries.push(name);
      if (entry.isDirectory()) await walk(absolute);
    }
  }
  await walk(root);
  return entries;
}

async function inspectPhysicalTree(root, allowMacFrameworkLinks) {
  const entries = [];
  const canonicalRoot = await realpath(root);
  async function walk(directory) {
    for (const entry of await readdir(directory, { withFileTypes: true })) {
      const absolute = resolve(directory, entry.name);
      const metadata = await lstat(absolute);
      const name = normalizePackagePath(relative(root, absolute));
      entries.push(name);
      if (metadata.isSymbolicLink()) {
        if (!allowMacFrameworkLinks || !isAllowedMacFrameworkLink(name)) {
          throw new Error(`Unexpected physical package link: ${name}`);
        }
        const rawTarget = await readlink(absolute);
        if (name === 'Applications' && rawTarget === '/Applications') continue;
        const target = resolve(dirname(absolute), rawTarget);
        const canonicalTarget = await realpath(target);
        if (relative(canonicalRoot, canonicalTarget).startsWith('..')) {
          throw new Error(`Physical package link escapes its package: ${name}`);
        }
      } else if (metadata.isDirectory()) {
        await walk(absolute);
      } else if (metadata.isFile()) {
        await scanPhysicalContent(absolute, name, isTextRuntimePath(name));
      }
    }
  }
  await walk(root);
  return entries;
}

async function scanPhysicalContent(path, packagePath, textual) {
  let overlap = Buffer.alloc(0);
  for await (const chunk of createReadStream(path, { highWaterMark: 64 * 1024 })) {
    const combined = Buffer.concat([overlap, chunk]);
    validateSecretContent(packagePath, combined.toString('latin1'));
    if (canonicalPackage) assertNoForbiddenProductionMarkers(packagePath, combined);
    if (textual) validateRuntimeContent(packagePath, combined.toString('utf8'));
    overlap = combined.subarray(
      Math.max(
        0,
        combined.length - Math.max(SECRET_SCAN_OVERLAP_BYTES, FORBIDDEN_MARKER_OVERLAP_BYTES),
      ),
    );
  }
}

function isAllowedMacFrameworkLink(path) {
  return (
    path === 'Applications' ||
    /\.framework\/(?:Versions\/Current|Resources|Libraries|Helpers|[^/]+)$/u.test(path)
  );
}

function isTextRuntimePath(path) {
  return /\.(?:c?js|mjs|json|html|css|txt|md|xml|plist|ya?ml)$/iu.test(path);
}

async function inspectFinalArtifacts(
  packageDirectory,
  mac,
  strict,
  requirement,
  expectedArtifact,
  unpackedReleaseMetadata,
) {
  const releaseDirectory = dirname(packageDirectory);
  const releaseFileNames = (await readdir(releaseDirectory, { withFileTypes: true }))
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name);
  const artifactNames = discoverFinalArtifactNames(releaseFileNames);
  if (requirement !== undefined) {
    validateSharedReleaseArtifacts(artifactNames, requirement, expectedArtifact);
  }
  const identityArtifactNames = finalArtifactNamesForIdentity(artifactNames, expectedArtifact);
  const artifacts = identityArtifactNames.map((name) => resolve(releaseDirectory, name));
  if (artifacts.length === 0) {
    return {
      summary: '0/0 expected final artifacts (directory package inspected)',
      artifacts,
    };
  }
  validatePhysicalEntries(artifactNames);
  const configuredSevenZip = mac ? null : configuredSevenZipPath();
  const hostSevenZip = mac ? null : findCommand(['7z', '7za']);
  const sevenZip = mac
    ? null
    : expectedArtifact.arch === 'arm64'
      ? (configuredSevenZip ?? hostSevenZip)
      : (bundledSevenZip() ?? configuredSevenZip ?? hostSevenZip);
  const ditto = mac ? findCommand(['ditto']) : null;
  const hdiutil = mac ? findCommand(['hdiutil']) : null;
  let inspected = 0;
  const methods = new Set();
  for (const artifact of artifacts) {
    const extractionRoot = resolve('tmp', 'package-inspection', basename(artifact));
    await rm(extractionRoot, { recursive: true, force: true });
    await mkdir(extractionRoot, { recursive: true });
    let inspectionRoot = extractionRoot;
    let detach = null;
    let extracted = null;
    let extractionArtifact = artifact;
    if (!mac && /\.exe$/iu.test(artifact)) {
      await extractNativePackage(
        artifact,
        extractionRoot,
        expectedArtifact.arch,
        expectedArtifact.packageMode,
      );
      extracted = { status: 0 };
      methods.add('tqpkg2');
    } else if (/\.zip$/iu.test(artifact) && ditto !== null) {
      extracted = spawnSync(ditto, ['-x', '-k', artifact, extractionRoot], {
        stdio: 'pipe',
        env: subprocessEnvironment,
      });
      methods.add('ditto');
    } else if (/\.dmg$/iu.test(artifact) && hdiutil !== null) {
      inspectionRoot = resolve(extractionRoot, 'mounted');
      await mkdir(inspectionRoot, { recursive: true });
      extracted = spawnSync(
        hdiutil,
        ['attach', '-readonly', '-nobrowse', '-mountpoint', inspectionRoot, artifact],
        { stdio: 'pipe', env: subprocessEnvironment },
      );
      detach = () =>
        spawnSync(hdiutil, ['detach', inspectionRoot], {
          stdio: 'pipe',
          env: subprocessEnvironment,
        });
      methods.add('hdiutil');
    } else if (sevenZip !== null) {
      extracted = await extractArchiveWithRetry(
        sevenZip,
        ['x', '-y', `-o${extractionRoot}`, extractionArtifact],
        extractionRoot,
      );
      methods.add(sevenZip);
    }
    try {
      if (extracted === null) continue;
      if (extracted.status !== 0) {
        const detail =
          extracted.error?.message ?? extracted.stderr?.toString().trim() ?? 'unknown error';
        console.warn(`Final-artifact extraction failed: ${detail}`);
        continue;
      }
      if (extractionArtifact !== artifact) await rm(extractionArtifact, { force: true });
      const entries = await inspectPhysicalTree(inspectionRoot, mac);
      validatePhysicalPackageEntries(entries, mac ? 'mac' : 'win', {
        macosOwner: macosOwnerPackage,
      });
      await inspectExtractedRuntime(
        inspectionRoot,
        mac,
        expectedArtifact.arch,
        unpackedReleaseMetadata,
      );
      inspected += 1;
    } finally {
      detach?.();
      await rm(extractionRoot, { recursive: true, force: true });
    }
  }
  const skipped = artifacts.length - inspected;
  const methodSummary = methods.size === 0 ? 'no supported extractor' : [...methods].join(', ');
  const summary = `${String(inspected)}/${String(artifacts.length)} recursively extracted with ${methodSummary}; ${String(skipped)} not claimed as extracted`;
  validateFinalArtifactInspection(artifacts.length, inspected, strict);
  return { summary, artifacts };
}

async function extractNativePackage(artifact, output, expectedArchitecture, expectedPackageMode) {
  const bytes = await readFile(artifact);
  if (canonicalPackage) {
    for (const marker of ['/TQ-CLEAN-STALE-SCHEMA2', '/TQ-DIAGNOSE-STALE-SCHEMA2']) {
      if (
        bytes.includes(Buffer.from(marker, 'ascii')) ||
        bytes.includes(Buffer.from(marker, 'utf16le'))
      ) {
        throw new Error(`Canonical Windows package contains cleanup feature marker: ${marker}`);
      }
    }
  }
  const exactPackage = parseTqpkg2(bytes, expectedArchitecture, {
    allowAcceptanceFaults: process.env.TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD === '1',
  });
  const peHeader = bytes.readUInt32LE(0x3c);
  const optionalHeader = peHeader + 24;
  const dataDirectory =
    bytes.readUInt16LE(optionalHeader) === 0x20b ? optionalHeader + 112 : optionalHeader + 96;
  const certificateOffset = bytes.readUInt32LE(dataDirectory + 32);
  const certificateSize = bytes.readUInt32LE(dataDirectory + 36);
  const signedEnd =
    certificateOffset === 0 && certificateSize === 0
      ? bytes.length
      : certificateOffset + certificateSize === bytes.length
        ? certificateOffset
        : -1;
  const footerOffset = signedEnd - 128;
  if (
    footerOffset < 256 ||
    bytes.subarray(footerOffset, footerOffset + 8).toString('binary') !== 'TQPKG2\0\0'
  ) {
    throw new Error('Windows final artifact is missing its TQPKG2 footer');
  }
  const footer = bytes.subarray(footerOffset);
  const offset = Number(footer.readBigUInt64LE(16));
  const size = Number(footer.readBigUInt64LE(24));
  const manifestSize = Number(footer.readBigUInt64LE(32));
  if (
    footer.readUInt32LE(8) !== 2 ||
    footer.readUInt32LE(12) !== 0 ||
    footer.subarray(104).some((byte) => byte !== 0) ||
    !Number.isSafeInteger(offset) ||
    !Number.isSafeInteger(size) ||
    !Number.isSafeInteger(manifestSize) ||
    offset + size !== footerOffset ||
    size <= 0 ||
    manifestSize <= 0 ||
    manifestSize > Math.min(size, 8 * 1024 * 1024)
  ) {
    throw new Error('Windows TQPKG2 footer range is invalid');
  }
  const pe = bytes.readUInt32LE(0x3c);
  const machine = expectedArchitecture === 'x64' ? 0x8664 : 0xaa64;
  if (
    bytes.readUInt32LE(pe) !== 0x0000_4550 ||
    bytes.readUInt16LE(pe + 4) !== machine ||
    bytes.readUInt16LE(pe + 24 + 68) !== 2
  ) {
    throw new Error('Windows native setup architecture or subsystem is invalid');
  }
  const packageBytes = bytes.subarray(offset, footerOffset);
  if (!createHash('sha256').update(packageBytes).digest().equals(footer.subarray(40, 72))) {
    throw new Error('Windows TQPKG2 package digest is invalid');
  }
  const manifestBytes = packageBytes.subarray(0, manifestSize);
  if (!createHash('sha256').update(manifestBytes).digest().equals(footer.subarray(72, 104))) {
    throw new Error('Windows TQPKG2 manifest digest is invalid');
  }
  const manifest = JSON.parse(manifestBytes.toString('utf8'));
  if (
    Buffer.from(canonicalJsonForInspection(manifest)).compare(manifestBytes) !== 0 ||
    manifest.schemaVersion !== 2 ||
    manifest.architecture !== expectedArchitecture ||
    !/^\d+\.\d+\.\d+$/u.test(manifest.version ?? '') ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(manifest.sourceTree ?? '') ||
    !['fresh', 'update', 'repair'].includes(manifest.packageMode) ||
    (manifest.packageMode === 'update') !== (manifest.predecessor !== null) ||
    !/^[0-9a-f]{64}$/u.test(manifest.treeSha256 ?? '') ||
    !/^[0-9a-f]{64}$/u.test(manifest.target?.releaseBuildDigest ?? '') ||
    !/^[0-9a-f]{64}$/u.test(manifest.target?.gatewaySha256 ?? '') ||
    !/^[0-9a-f]{64}$/u.test(manifest.target?.ownerSha256 ?? '') ||
    !/^[0-9a-f]{64}$/u.test(manifest.target?.recoveryLauncherSha256 ?? '') ||
    !Array.isArray(manifest.files) ||
    manifest.files.length === 0 ||
    manifest.files.length > 200_000
  ) {
    throw new Error('Windows TQPKG2 manifest is not canonical');
  }
  if (manifest.packageMode !== expectedPackageMode) {
    throw new Error('Windows TQPKG2 package mode does not match unpacked release metadata');
  }
  const folded = new Set();
  const tree = createHash('sha256');
  let expectedOffset = manifestSize;
  let treeBytes = 0;
  for (const file of manifest.files) {
    const path = String(file.path ?? '');
    if (
      !path ||
      path.includes('\\') ||
      path.includes(':') ||
      path.length > 1024 ||
      !/^[\x20-\x7e]+$/u.test(path) ||
      path
        .split('/')
        .some(
          (part) =>
            !part ||
            part === '.' ||
            part === '..' ||
            part.endsWith('.') ||
            part.endsWith(' ') ||
            /^(?:con|prn|aux|nul|com[1-9]|lpt[1-9])(?:\.|$)/iu.test(part) ||
            /[<>"|?*]/u.test(part),
        )
    )
      throw new Error('Windows TQPKG2 path is invalid');
    const key = path.toLowerCase();
    if (folded.has(key)) throw new Error('Windows TQPKG2 has a case collision');
    folded.add(key);
    if (
      file.mode !== 0 ||
      !Number.isSafeInteger(file.size) ||
      file.size < 0 ||
      file.size > 4 * 1024 ** 3 ||
      !Number.isSafeInteger(file.blockOffset) ||
      file.blockOffset !== expectedOffset ||
      !Number.isSafeInteger(file.blockSize) ||
      file.blockSize <= 0 ||
      !/^[0-9a-f]{64}$/u.test(file.sha256 ?? '')
    ) {
      throw new Error('Windows TQPKG2 block framing is invalid');
    }
    expectedOffset += file.blockSize;
    treeBytes += file.size;
    if (treeBytes > 16 * 1024 ** 3 || expectedOffset > size)
      throw new Error('Windows TQPKG2 size limit is invalid');
    for (const value of [path, String(file.mode), String(file.size), file.sha256]) {
      const framed = Buffer.from(value);
      const length = Buffer.alloc(8);
      length.writeBigUInt64LE(BigInt(framed.length));
      tree.update(length).update(framed);
    }
    const compressed = packageBytes.subarray(file.blockOffset, file.blockOffset + file.blockSize);
    const content = zstdDecompressSync(compressed, { maxOutputLength: file.size });
    if (
      content.length !== file.size ||
      createHash('sha256').update(content).digest('hex') !== file.sha256
    )
      throw new Error('Windows TQPKG2 file digest is invalid');
    const destination = resolve(output, ...path.split('/'));
    if (!destination.startsWith(`${resolve(output)}${sep}`))
      throw new Error('Windows TQPKG2 path escaped extraction');
    await mkdir(dirname(destination), { recursive: true });
    await writeFile(destination, content, { flag: 'wx' });
  }
  if (expectedOffset !== size || tree.digest('hex') !== manifest.treeSha256)
    throw new Error('Windows TQPKG2 tree digest is invalid');
  const ownerManifest = JSON.parse(
    await readFile(resolve(output, 'resources/keyboard-owner-release-v1.json'), 'utf8'),
  );
  bindTqpkg2OwnerManifest(exactPackage.manifest, ownerManifest);
  const role = (name) => ownerManifest.roles?.find((value) => value.role === name)?.sha256;
  if (
    ownerManifest.version !== manifest.version ||
    ownerManifest.architecture !== manifest.architecture ||
    ownerManifest.sourceCommit !== manifest.sourceCommit ||
    ownerManifest.sourceTree !== manifest.sourceTree ||
    ownerManifest.releaseBuildDigest !== manifest.target.releaseBuildDigest ||
    role('gateway') !== manifest.target.gatewaySha256 ||
    role('owner') !== manifest.target.ownerSha256 ||
    role('recovery-launcher') !== manifest.target.recoveryLauncherSha256 ||
    !predecessorMatches(ownerManifest.predecessor, manifest.predecessor)
  ) {
    throw new Error('Windows TQPKG2 identity is not bound to the owner release manifest');
  }
}

function predecessorMatches(owner, embedded) {
  if (owner == null || embedded == null) return owner == null && embedded == null;
  return ['version', 'releaseBuildDigest', 'gatewaySha256', 'ownerSha256'].every(
    (key) => owner[key] === embedded[key],
  );
}

function canonicalJsonForInspection(value) {
  if (value === null || typeof value === 'string' || typeof value === 'boolean')
    return JSON.stringify(value);
  if (typeof value === 'number') return String(value);
  if (Array.isArray(value)) return `[${value.map(canonicalJsonForInspection).join(',')}]`;
  return `{${Object.keys(value)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${canonicalJsonForInspection(value[key])}`)
    .join(',')}}`;
}

async function inspectExtractedRuntime(root, mac, expectedArch, unpackedReleaseMetadata) {
  const extractedResources = mac
    ? resolve(root, 'Talking Quill.app', 'Contents', 'Resources')
    : resolve(root, 'resources');
  const requiredNativePaths = [
    mac
      ? resolve(root, 'Talking Quill.app', 'Contents', 'MacOS', 'Talking Quill')
      : resolve(root, 'Talking Quill.exe'),
    resolve(
      extractedResources,
      'helper',
      mac ? 'talking-quill-helper' : 'talking-quill-helper.exe',
    ),
    ...(mac
      ? []
      : [
          resolve(extractedResources, 'helper/talking-quill-keyboard-owner.exe'),
          resolve(extractedResources, 'helper/talking-quill-update-recovery-launcher.exe'),
        ]),
    resolve(
      extractedResources,
      'app.asar.unpacked/node_modules/better-sqlite3/build/Release/better_sqlite3.node',
    ),
    resolve(
      extractedResources,
      `app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/${mac ? 'darwin' : 'win32'}/${expectedArch}/onnxruntime_binding.node`,
    ),
    resolve(
      extractedResources,
      `app.asar.unpacked/node_modules/onnxruntime-node/bin/napi-v3/${mac ? 'darwin' : 'win32'}/${expectedArch}/${mac ? 'libonnxruntime.1.21.0.dylib' : 'onnxruntime.dll'}`,
    ),
    ...(mac && macosOwnerPackage
      ? [
          resolve(
            root,
            'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
          ),
          resolve(root, 'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge'),
        ]
      : []),
  ];
  const extractedHelper = requiredNativePaths[1];
  if (extractedHelper === undefined) throw new Error('Extracted helper path is unavailable');
  await verifyHelperBuildContract(extractedHelper, { windows: !mac });
  await verifyNativeSourceIdentity(extractedHelper, sourceIdentity);
  if (mac && macosOwnerPackage) {
    const extractedOwner = resolve(
      root,
      'Talking Quill.app/Contents/Library/LoginItems/Talking Quill Keyboard Owner.app/Contents/MacOS/talking-quill-keyboard-owner',
    );
    const extractedBridge = resolve(
      root,
      'Talking Quill.app/Contents/MacOS/talking-quill-macos-service-bridge',
    );
    await verifyOwnerBuildContract(extractedOwner);
    await verifyNativeSourceIdentity(extractedOwner, sourceIdentity);
    await verifyMacosServiceBridgeBuildContract(extractedBridge);
    await verifyNativeSourceIdentity(extractedBridge, sourceIdentity);
    const extractedInventory = await inspectNativeTree(root, {
      platform: 'mac',
      architecture: expectedArch,
      exceptions: nativeArchitectureExceptions(true),
    });
    await verifyCompleteNativeRoleInventory(
      extractedInventory.map((entry) => resolve(root, entry.path)),
      [extractedHelper, extractedOwner, extractedBridge],
    );
  }
  if (!mac) {
    const roleDirectory = resolve(extractedResources, 'helper');
    const extractedOwner = resolve(roleDirectory, 'talking-quill-keyboard-owner.exe');
    const extractedRecoveryLauncher = resolve(
      roleDirectory,
      'talking-quill-update-recovery-launcher.exe',
    );
    await verifyOwnerBuildContract(extractedOwner);
    await verifyNativeSourceIdentity(extractedOwner, sourceIdentity);
    await verifyWindowsUpdateRecoveryLauncherBuildContract(extractedRecoveryLauncher);
    await verifyNativeSourceIdentity(extractedRecoveryLauncher, sourceIdentity);
    const extractedInventory = await inspectNativeTree(root, {
      platform: 'win',
      architecture: expectedArch,
      exceptions: nativeArchitectureExceptions(false),
    });
    await verifyCompleteNativeRoleInventory(
      extractedInventory.map((entry) => resolve(root, entry.path)),
      [extractedHelper, extractedOwner, extractedRecoveryLauncher],
    );
  }
  if (!mac || macosOwnerPackage) {
    const extractedMetadataPath = resolve(extractedResources, RELEASE_PACKAGE_METADATA_NAME);
    if (unpackedReleaseMetadata === null) {
      throw new Error('Unpacked release metadata is unavailable for final-artifact comparison');
    }
    verifyMatchingPackageReleaseMetadataBytes(
      unpackedReleaseMetadata,
      await readFile(extractedMetadataPath),
    );
    await verifySerializedPackageReleaseMetadata(extractedMetadataPath, root, {
      version: expectedVersion,
      platform: mac ? 'mac' : 'win',
      architecture: expectedArch,
    });
  }
  for (const path of requiredNativePaths) {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size === 0) {
      throw new Error(`Extracted native runtime is missing or invalid: ${path}`);
    }
    const native = await readNativeArchitectures(path);
    if (
      native === null ||
      (mac ? native.format === 'pe' : native.format !== 'pe') ||
      native.architectures.length !== 1 ||
      native.architectures[0] !== expectedArch
    ) {
      throw new Error(`Extracted required runtime is not a ${expectedArch} native image: ${path}`);
    }
  }
  const extractedNativeEntries = await inspectNativeTree(root, {
    platform: mac ? 'mac' : 'win',
    architecture: expectedArch,
    exceptions: nativeArchitectureExceptions(mac),
  });
  if (extractedNativeEntries.length < requiredNativePaths.length) {
    throw new Error('Extracted artifact did not expose every required native runtime image');
  }
  const extractedResourceEntries = await walkResources(extractedResources);
  validateResourceEntries(extractedResourceEntries, mac ? 'mac' : 'win', {
    architecture: expectedArch,
    macosOwner: macosOwnerPackage,
    windowsInstalledAcceptance,
  });
  const extractedAsar = resolve(extractedResources, 'app.asar');
  const entries = listPackage(extractedAsar).map(normalizePackagePath);
  validateAsarEntries(entries, {
    platform: mac ? 'mac' : 'win',
    architecture: expectedArch,
  });
  inspectAsarContent(extractedAsar, entries, 'extracted app.asar', 'Extracted final artifact ASAR');
  validatePackagedWhisper(extractedAsar);
}

function inspectAsarContent(archivePath, entries, packageLabel, policyLabel = 'ASAR') {
  for (const { entry, bytes } of extractRegularAsarFiles(archivePath, entries, policyLabel)) {
    if (canonicalPackage) {
      assertNoForbiddenProductionMarkers(`${packageLabel}/${entry}`, bytes);
    }
    validateSecretContent(entry, bytes.toString('latin1'));
    if (!isTextRuntimePath(entry)) continue;
    const source = bytes.toString('utf8');
    validateRuntimeContent(entry, source);
    if (!/\.(?:c?js|mjs)$/u.test(entry)) continue;
    for (const marker of testHarnessMarkers) {
      if (source.includes(marker)) {
        throw new Error(`Packaged production graph contains test marker ${marker} in ${entry}`);
      }
    }
  }
}

function validatePackagedWhisper(archivePath) {
  const packagedBootstrap = extractFile(
    archivePath,
    'out/workers/whisper-bootstrap.cjs'.replaceAll('/', sep),
    false,
  ).toString('utf8');
  const packagedPayload = extractFile(
    archivePath,
    'out/workers/whisper-payload.cjs'.replaceAll('/', sep),
    false,
  ).toString('utf8');
  const guardInstallation = packagedBootstrap.indexOf('installWorkerNetworkGuard();');
  const payloadLoad = packagedBootstrap.indexOf('("./whisper-payload.cjs")');
  if (guardInstallation < 0 || payloadLoad <= guardInstallation) {
    throw new Error('Packaged Whisper bootstrap does not guard the production payload.');
  }
  for (const forbidden of ['@huggingface/transformers', 'onnxruntime-node', 'zod']) {
    if (packagedBootstrap.includes(forbidden)) {
      throw new Error(`Packaged Whisper bootstrap contains ${forbidden}.`);
    }
  }
  if (!packagedPayload.includes('onnxruntime-node')) {
    throw new Error('Packaged Whisper payload does not load ONNX Runtime.');
  }
}

function nativeArchitectureExceptions(mac) {
  return mac ? {} : { 'resources/elevate.exe': 'x86' };
}

async function emitArtifactPaths(paths) {
  const outputPath = process.env.GITHUB_OUTPUT;
  if (outputPath === undefined) return;
  await appendFile(
    outputPath,
    `artifact_paths<<TALKING_QUILL_ARTIFACT_PATHS\n${paths.join('\n')}\nTALKING_QUILL_ARTIFACT_PATHS\n`,
    'utf8',
  );
}

async function extractArchiveWithRetry(command, arguments_, extractionRoot) {
  const attempts = 16;
  let result = null;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    await rm(extractionRoot, { recursive: true, force: true });
    await mkdir(extractionRoot, { recursive: true });
    result = spawnSync(command, arguments_, {
      stdio: 'pipe',
      env: subprocessEnvironment,
    });
    if (result.status === 0) return result;
    if (attempt < attempts) await new Promise((resolveDelay) => setTimeout(resolveDelay, 2_000));
  }
  const detail = result?.error?.message ?? result?.stderr?.toString().trim() ?? 'unknown error';
  console.warn(`Final-artifact extraction failed after ${String(attempts)} attempts: ${detail}`);
  return result;
}

function bundledSevenZip() {
  try {
    const module = require('7zip-bin');
    return typeof module.path7za === 'string' && existsSync(module.path7za) ? module.path7za : null;
  } catch {
    return null;
  }
}

function configuredSevenZipPath() {
  const configured = process.env.TALKING_QUILL_7ZIP_PATH;
  if (configured === undefined) return null;
  const executable = resolve(configured);
  if (!existsSync(executable)) {
    throw new Error(`Configured 7-Zip extractor does not exist: ${executable}`);
  }
  return executable;
}

function findCommand(commands) {
  for (const command of commands) {
    const result = spawnSync(process.platform === 'win32' ? 'where' : 'which', [command], {
      stdio: 'ignore',
      env: subprocessEnvironment,
    });
    if (result.status === 0) return command;
  }
  return null;
}
