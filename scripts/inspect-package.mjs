import { spawnSync } from 'node:child_process';
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
} from 'node:fs/promises';
import { basename, dirname, relative, resolve, sep } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { extractFile, listPackage, statFile } from '@electron/asar';
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
import { inspectNativeTree, readNativeArchitectures } from './native-architecture.mjs';
import {
  artifactUploadPaths,
  verifyArtifactProvenanceManifest,
  writeArtifactProvenanceManifest,
} from './artifact-provenance.mjs';

const require = createRequire(import.meta.url);
const invocationDirectory = process.cwd();
const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const packageArgument = process.argv
  .slice(2)
  .find((argument) => argument !== '--' && !argument.startsWith('--'));
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
    'Strict final-artifact inspection requires --artifacts-required=none|nsis|dmg-zip or TALKING_QUILL_PACKAGE_ARTIFACTS_REQUIRED',
  );
}
const packageRoot = resolve(invocationDirectory, packageArgument ?? 'release/win-unpacked');
process.chdir(repositoryRoot);
const appManifest = JSON.parse(await readFile(resolve('app/package.json'), 'utf8'));
const expectedVersion = appManifest.version;
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
for (const entry of asarEntries) {
  const stat = statFile(asarPath, entry.replaceAll('/', sep), false);
  if (stat.files !== undefined || stat.unpacked === true) continue;
  const bytes = extractFile(asarPath, entry.replaceAll('/', sep));
  validateSecretContent(entry, bytes.toString('latin1'));
  if (isTextRuntimePath(entry)) {
    const source = bytes.toString('utf8');
    validateRuntimeContent(entry, source);
    if (/\.(?:c?js|mjs)$/u.test(entry)) {
      for (const marker of testHarnessMarkers) {
        if (source.includes(marker)) {
          throw new Error(`Packaged production graph contains test marker ${marker} in ${entry}`);
        }
      }
    }
  }
}
const packagedBootstrap = extractFile(
  asarPath,
  'out/workers/whisper-bootstrap.cjs'.replaceAll('/', sep),
).toString('utf8');
const packagedPayload = extractFile(
  asarPath,
  'out/workers/whisper-payload.cjs'.replaceAll('/', sep),
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
for (const entry of asarEntries) {
  if (entry.length > 0 && 'link' in statFile(asarPath, entry.replaceAll('/', sep))) {
    throw new Error(`ASAR symlink is not allowed: ${entry}`);
  }
}
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
validateResourceEntries(resourceEntries, isMacBundle ? 'mac' : 'win');
const physicalEntries = await inspectPhysicalTree(packageRoot, isMacBundle);
validatePhysicalPackageEntries(physicalEntries, isMacBundle ? 'mac' : 'win');
const unpackedNativeEntries = await inspectNativeTree(packageRoot, {
  platform: boundPlatform,
  architecture: boundArch,
  exceptions: nativeArchitectureExceptions(isMacBundle),
});
if (unpackedNativeEntries.length === 0) {
  throw new Error('No native executable images were discovered in the unpacked package');
}
const artifactEvidence = await inspectFinalArtifacts(
  packageRoot,
  isMacBundle,
  strictArtifactInspection,
  artifactRequirement,
  { version: expectedVersion, platform: boundPlatform, arch: boundArch },
);
const noticeCheck = spawnSync(process.execPath, ['scripts/generate-notices.mjs', '--check'], {
  stdio: 'inherit',
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
    if (textual) validateRuntimeContent(packagePath, combined.toString('utf8'));
    overlap = combined.subarray(Math.max(0, combined.length - SECRET_SCAN_OVERLAP_BYTES));
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

async function inspectFinalArtifacts(packageDirectory, mac, strict, requirement, expectedArtifact) {
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
  const sevenZip = bundledSevenZip() ?? findCommand(['7z', '7za']);
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
    if (sevenZip !== null) {
      extracted = await extractArchiveWithRetry(
        sevenZip,
        ['x', '-y', `-o${extractionRoot}`, artifact],
        extractionRoot,
      );
      methods.add(sevenZip);
    } else if (/\.zip$/iu.test(artifact) && ditto !== null) {
      extracted = spawnSync(ditto, ['-x', '-k', artifact, extractionRoot], { stdio: 'pipe' });
      methods.add('ditto');
    } else if (/\.dmg$/iu.test(artifact) && hdiutil !== null) {
      inspectionRoot = resolve(extractionRoot, 'mounted');
      await mkdir(inspectionRoot, { recursive: true });
      extracted = spawnSync(
        hdiutil,
        ['attach', '-readonly', '-nobrowse', '-mountpoint', inspectionRoot, artifact],
        { stdio: 'pipe' },
      );
      detach = () => spawnSync(hdiutil, ['detach', inspectionRoot], { stdio: 'pipe' });
      methods.add('hdiutil');
    }
    try {
      if (extracted?.status !== 0) continue;
      if (extracted === null) continue;
      const entries = await inspectPhysicalTree(inspectionRoot, mac);
      validatePhysicalPackageEntries(entries, mac ? 'mac' : 'win');
      await inspectExtractedRuntime(inspectionRoot, mac, expectedArtifact.arch);
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

async function inspectExtractedRuntime(root, mac, expectedArch) {
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
  ];
  for (const path of requiredNativePaths) {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.isSymbolicLink() || metadata.size === 0) {
      throw new Error(`Extracted native runtime is missing or invalid: ${path}`);
    }
    const native = await readNativeArchitectures(path);
    if (
      native === null ||
      (mac ? native.format === 'pe' : native.format !== 'pe') ||
      !native.architectures.includes(expectedArch)
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
  validateResourceEntries(extractedResourceEntries, mac ? 'mac' : 'win');
  const extractedAsar = resolve(extractedResources, 'app.asar');
  const entries = listPackage(extractedAsar).map(normalizePackagePath);
  validateAsarEntries(entries);
  for (const entry of entries) {
    const metadata = statFile(extractedAsar, entry.replaceAll('/', sep));
    if (entry.length > 0 && 'link' in metadata) {
      throw new Error(`Extracted final artifact contains an ASAR link: ${entry}`);
    }
    if (isTextRuntimePath(entry)) {
      validateRuntimeContent(
        entry,
        extractFile(extractedAsar, entry.replaceAll('/', sep)).toString('utf8'),
      );
    }
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
    result = spawnSync(command, arguments_, { stdio: 'pipe' });
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
    // The ARM64 build cannot decode the NSIS compression methods used by electron-builder.
    // Windows ARM64 provides x64 emulation, so use the bundled x64 extractor there.
    const executable =
      process.platform === 'win32' && process.arch === 'arm64'
        ? resolve(dirname(require.resolve('7zip-bin/package.json')), 'win', 'x64', '7za.exe')
        : module.path7za;
    return typeof executable === 'string' && existsSync(executable) ? executable : null;
  } catch {
    return null;
  }
}

function findCommand(commands) {
  for (const command of commands) {
    const result = spawnSync(process.platform === 'win32' ? 'where' : 'which', [command], {
      stdio: 'ignore',
    });
    if (result.status === 0) return command;
  }
  return null;
}
