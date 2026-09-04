import { createHash, randomBytes } from 'node:crypto';
import { lstat, mkdir, open, readFile, rm, writeFile } from 'node:fs/promises';
import { relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';
import { currentSourceIdentity } from './source-identity.mjs';
import { parseTqpkg2 } from './tqpkg2.mjs';
import { buildWindowsInstalledAcceptanceArtifacts } from './windows-installed-acceptance-native-build.mjs';
import {
  buildInstalledAcceptanceKit,
  validateCanonicalRelease,
} from './build-windows-installed-acceptance-kit.mjs';
import { assertNoLinkPath } from './windows-installed-acceptance-bundle.mjs';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_RUN_MS,
} from './windows-installed-acceptance.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { cleanupAcceptanceNative } from './windows-installed-acceptance-native-publication.mjs';

const repositoryRoot = resolve(fileURLToPath(new URL('..', import.meta.url)));
const HEX_32 = /^[0-9a-f]{64}$/u;

export function installedAcceptanceBuildEnvironment(environment = process.env, additions = {}) {
  return sanitizedSubprocessEnvironment(environment, additions);
}

export async function buildWindowsInstalledAcceptanceInputs(options, dependencies = {}) {
  if (options.architecture !== 'x64') {
    throw new Error('The first-party installed-acceptance producer requires Windows x64');
  }
  const outputRoot = resolve(options.outputRoot ?? 'tmp/windows-installed-acceptance/producer');
  requireBelowTmp(outputRoot);
  await ensureSafeOutputParent(resolve(outputRoot, '..'));
  await requireAbsent(outputRoot);
  await mkdir(outputRoot, { recursive: false, mode: 0o700 });
  await assertNoLinkPath(outputRoot, { directory: true });
  let nativePublication;
  let retainNativePublication = false;
  let completed = false;
  let returnValue;
  let primaryError;
  let cleanupError;
  try {
    const validateRelease = dependencies.validateCanonicalRelease ?? validateCanonicalRelease;
    const canonical = await validateRelease(options);
    if (canonical.descriptor.architecture !== options.architecture) {
      throw new Error('Canonical RELEASE architecture differs from the producer target');
    }
    const readSourceIdentity = dependencies.currentSourceIdentity ?? currentSourceIdentity;
    const sourceIdentity = readSourceIdentity({
      repositoryRoot,
      requireClean: process.env.NODE_ENV !== 'test',
    });
    if (
      sourceIdentity.sourceCommit !== canonical.descriptor.sourceCommit ||
      sourceIdentity.sourceTree !== canonical.descriptor.sourceTree
    ) {
      throw new Error('Acceptance build checkout differs from the canonical source identity');
    }
    const parseCanonicalPackage = dependencies.parseTqpkg2 ?? parseTqpkg2;
    const parsed = parseCanonicalPackage(canonical.installerBytes, options.architecture);
    const canonicalRoot = resolve(outputRoot, 'canonical-unpacked');
    await extractCanonicalContents(parsed.contents, canonicalRoot);
    const canonicalMetadataPath = resolve(
      canonicalRoot,
      'resources/keyboard-owner-release-v1.json',
    );
    const canonicalMetadataBytes = await readFile(canonicalMetadataPath);
    const canonicalMetadata = JSON.parse(canonicalMetadataBytes.toString('utf8'));
    assertCanonicalMetadata(canonicalMetadata, canonical.descriptor);

    const buildId = requireHex(options.buildId ?? randomBytes(32).toString('hex'), 'build ID');
    const runWindow = createRunWindow(options);
    const workspace = Object.freeze({
      repositoryRoot,
      outputRoot,
      canonicalRoot,
      canonical,
      canonicalMetadata,
      canonicalMetadataSha256: createHash('sha256').update(canonicalMetadataBytes).digest('hex'),
      buildId,
      runWindow,
      predecessorEnvironment: predecessorEnvironment(canonicalMetadata, canonicalRoot),
    });
    const produceArtifacts =
      dependencies.produceArtifacts ??
      ((producerOptions, producerContext) =>
        buildWindowsInstalledAcceptanceArtifacts(producerOptions, producerContext));
    const produced = await produceArtifacts(options, workspace, dependencies);
    validateProducedArtifacts(produced);
    nativePublication = produced.nativePublication;

    const requestPayloads = {};
    const requestNonces = {};
    for (const invocation of ACCEPTANCE_REQUEST_SCHEDULE) {
      const secret = randomBytes(16).toString('hex');
      requestNonces[invocation.invocationId] = randomBytes(32).toString('hex');
      requestPayloads[invocation.invocationId] = {
        readinessPipe: `\\\\.\\pipe\\TalkingQuill.InstalledReadiness.${secret}`,
        launchCorrelation: randomBytes(32).toString('hex'),
        physicalObservation: invocation.command === 'manual-physical-observation',
        automationValidation: [
          'gateway-reconnect-arm',
          'electron-crash-arm',
          'supplemental-synthetic-observation',
        ].includes(invocation.command),
        automationArmedPipe: needsArmedPipe(invocation.command)
          ? `\\\\.\\pipe\\TalkingQuill.AutomationArmed.${randomBytes(16).toString('hex')}`
          : null,
        automationCase:
          invocation.command === 'supplemental-synthetic-observation'
            ? 'general'
            : ['gateway-reconnect-arm', 'electron-crash-arm'].includes(invocation.command)
              ? 'lifecycle'
              : null,
        lifecycleUserData: null,
        heartbeatDurationMs: invocation.command === 'heartbeat-120s' ? 120_000 : 6_250,
      };
    }
    const config = {
      architecture: options.architecture,
      artifacts: {
        predecessor: produced.predecessor,
        candidate: produced.candidate,
        fresh: produced.predecessor,
        repair: produced.repair,
        fault: produced.faults.published,
        faults: produced.faults,
      },
      acceptance: {
        buildId,
        sourceRevision: canonical.descriptor.sourceCommit.slice(0, 12),
        runWindow,
        requestPayloads,
        requestNonces,
        buildManifestPath: produced.buildManifestPath,
        buildManifestSha256: await fileHash(produced.buildManifestPath),
        manifestPublicKeySpkiBase64url: produced.manifestPublicKeySpkiBase64url,
        validationPublicKeySpkiBase64url: produced.validationPublicKeySpkiBase64url,
        validationChainHeadSha256: produced.validationChainHeadSha256,
        signerSha256: produced.signerSha256,
        syntheticSenderPath: produced.syntheticSenderPath,
        syntheticSenderSha256: await fileHash(produced.syntheticSenderPath),
        acceptanceBrokerPath: produced.acceptanceBrokerPath,
        acceptanceBrokerSha256: produced.acceptanceBrokerSha256,
        acceptanceBootstrapPath: produced.acceptanceBootstrapPath,
        acceptanceBootstrapSha256: produced.acceptanceBootstrapSha256,
        trustedLauncherPath: produced.trustedLauncherPath,
        trustedLauncherSha256: await fileHash(produced.trustedLauncherPath),
        syntheticSenderArguments: produced.syntheticSenderArguments ?? [],
      },
      outputPath: '../evidence.json',
    };
    const configPath = resolve(outputRoot, 'kit-input.json');
    await writeFile(configPath, `${JSON.stringify(config)}\n`, {
      flag: 'wx',
      mode: 0o600,
    });
    const assembler = dependencies.buildInstalledAcceptanceKit ?? buildInstalledAcceptanceKit;
    const kit =
      options.assemble === false
        ? null
        : await assembler({
            ...options,
            signerPath: produced.signerPath,
            signerSha256: produced.signerSha256,
            configPath,
            outputRoot: options.kitOutputRoot,
            bundlePath: options.bundlePath,
          });
    retainNativePublication = options.assemble === false;
    completed = true;
    returnValue = Object.freeze({
      configPath,
      config,
      kit,
      outputRoot,
      nativePublication: retainNativePublication ? nativePublication : null,
    });
  } catch (error) {
    nativePublication ??= error?.nativePublication;
    primaryError = error;
  } finally {
    try {
      if (!retainNativePublication) {
        const cleanup = dependencies.cleanupAcceptanceNative ?? cleanupAcceptanceNative;
        if (nativePublication !== undefined) await cleanup(nativePublication);
      }
      if (!completed && cleanupError === undefined) {
        await rm(outputRoot, { recursive: true, force: true });
      }
    } catch (error) {
      cleanupError = error;
    }
  }
  if (primaryError !== undefined && cleanupError !== undefined) {
    throw new AggregateError(
      [primaryError, cleanupError],
      'Acceptance producer and cleanup failed',
    );
  }
  if (primaryError !== undefined) throw primaryError;
  if (cleanupError !== undefined) throw cleanupError;
  return returnValue;
}

function predecessorEnvironment(metadata, root) {
  const role = (name) => metadata.roles.find((entry) => entry.role === name);
  return Object.freeze({
    TALKING_QUILL_PACKAGE_MODE: 'update',
    TALKING_QUILL_PREDECESSOR_VERSION: metadata.version,
    TALKING_QUILL_PREDECESSOR_RELEASE_BUILD: metadata.releaseBuildDigest,
    TALKING_QUILL_PREDECESSOR_GATEWAY_SHA256: role('gateway').sha256,
    TALKING_QUILL_PREDECESSOR_OWNER_SHA256: role('owner').sha256,
    TALKING_QUILL_PREDECESSOR_GATEWAY_PATH: resolve(root, role('gateway').path),
    TALKING_QUILL_PREDECESSOR_OWNER_PATH: resolve(root, role('owner').path),
  });
}

function assertCanonicalMetadata(metadata, descriptor) {
  if (
    metadata.version !== descriptor.version ||
    metadata.architecture !== descriptor.architecture ||
    metadata.platform !== 'win' ||
    metadata.packageMode !== 'fresh' ||
    metadata.sourceCommit !== descriptor.sourceCommit ||
    metadata.sourceTree !== descriptor.sourceTree ||
    metadata.freshInstall !== true ||
    metadata.predecessor !== null
  ) {
    throw new Error('Canonical extracted owner metadata is invalid');
  }
}

function validateProducedArtifacts(value) {
  if (value === null || typeof value !== 'object')
    throw new Error('Produced artifacts are missing');
  const faultNames = Object.keys(value.faults ?? {});
  if (canonicalAcceptanceJson(faultNames) !== canonicalAcceptanceJson(ACCEPTANCE_FAULT_PHASES)) {
    throw new Error('Heavy build must produce all ten ordered fault artifacts');
  }
  for (const name of [
    'predecessor',
    'signerPath',
    'signerSha256',
    'acceptanceBrokerPath',
    'acceptanceBrokerSha256',
    'acceptanceBootstrapPath',
    'acceptanceBootstrapSha256',
    'candidate',
    'repair',
    'buildManifestPath',
    'manifestPublicKeySpkiBase64url',
    'validationPublicKeySpkiBase64url',
    'validationChainHeadSha256',
    'syntheticSenderPath',
    'trustedLauncherPath',
    'nativePublication',
  ]) {
    if (value[name] === undefined) throw new Error(`Produced acceptance input is missing: ${name}`);
  }
  for (const [phase, artifact] of Object.entries(value.faults)) {
    if (artifact.validationEvidencePath === undefined) {
      throw new Error(`Fault ${phase} lacks isolated namespace validation evidence`);
    }
  }
}

async function extractCanonicalContents(contents, root) {
  await mkdir(root, { recursive: true, mode: 0o700 });
  for (const [name, bytes] of contents) {
    const output = resolve(root, ...name.split('/'));
    const local = relative(root, output);
    if (local === '' || local === '..' || local.startsWith(`..${sep}`) || local.includes(':')) {
      throw new Error('Canonical package extraction path escaped its root');
    }
    await mkdir(resolve(output, '..'), { recursive: true, mode: 0o700 });
    await writeFile(output, bytes, { flag: 'wx', mode: 0o600 });
  }
}

function createRunWindow(options) {
  const notBeforeMs = Number(options.notBeforeMs ?? Date.now() + 5 * 60_000);
  const expiresAtMs = Number(options.expiresAtMs ?? notBeforeMs + MAX_ACCEPTANCE_RUN_MS);
  if (
    !Number.isSafeInteger(notBeforeMs) ||
    !Number.isSafeInteger(expiresAtMs) ||
    expiresAtMs - notBeforeMs !== MAX_ACCEPTANCE_RUN_MS
  ) {
    throw new Error('Acceptance run window must equal the fixed maximum run duration');
  }
  return Object.freeze({ notBeforeMs, expiresAtMs, maxTotalRunMs: MAX_ACCEPTANCE_RUN_MS });
}

function needsArmedPipe(command) {
  return [
    'gateway-reconnect-arm',
    'electron-crash-arm',
    'supplemental-synthetic-observation',
    'login-marker',
    'manual-physical-observation',
  ].includes(command);
}

async function fileHash(path) {
  const absolute = resolve(path);
  await assertNoLinkPath(absolute, { file: true });
  const handle = await open(absolute, 'r');
  try {
    const before = await handle.stat();
    const bytes = await handle.readFile();
    const after = await handle.stat();
    const pathAfter = await lstat(absolute);
    if (
      !before.isFile() ||
      before.nlink !== 1 ||
      before.dev !== after.dev ||
      before.ino !== after.ino ||
      before.size !== bytes.length ||
      before.size !== after.size ||
      pathAfter.dev !== before.dev ||
      pathAfter.ino !== before.ino ||
      pathAfter.size !== before.size
    ) {
      throw new Error('Acceptance producer input changed while hashing');
    }
    return createHash('sha256').update(bytes).digest('hex');
  } finally {
    await handle.close();
  }
}

function requireHex(value, label) {
  if (!HEX_32.test(value ?? '')) throw new Error(`Acceptance ${label} is invalid`);
  return value;
}

function requireBelowTmp(path) {
  const local = relative(resolve(repositoryRoot, 'tmp'), path);
  if (local === '' || local === '..' || local.startsWith(`..${sep}`) || local.includes(':')) {
    throw new Error('Acceptance producer output must stay below tmp');
  }
}

async function ensureSafeOutputParent(parent) {
  const tmpRoot = resolve(repositoryRoot, 'tmp');
  await assertNoLinkPath(tmpRoot, { directory: true });
  const local = relative(tmpRoot, parent);
  let current = tmpRoot;
  for (const component of local.split(/[\\/]/u).filter(Boolean)) {
    current = resolve(current, component);
    await mkdir(current, { recursive: false, mode: 0o700 }).catch((error) => {
      if (error?.code !== 'EEXIST') throw error;
    });
    await assertNoLinkPath(current, { directory: true });
  }
}

async function requireAbsent(path) {
  try {
    await lstat(path);
    throw new Error('Acceptance producer output already exists');
  } catch (error) {
    if (error?.code !== 'ENOENT') throw error;
  }
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

const usage =
  'Usage: node scripts/build-windows-installed-acceptance-inputs.mjs --release RELEASE.json --release-sha256 <sha256> --provenance artifact-provenance.json --provenance-sha256 <sha256> --source <git-root> --request-private-key <pkcs8-der> --signer <native-signer> --signer-sha256 <sha256> --manifest-private-key <pkcs8-der> --update-private-key <protected-p256-pkcs8-der> --validation-private-key <pkcs8-der> --not-before-ms <ms> --expires-at-ms <ms> [--build-id <hex>] [--output tmp/path] [--kit-output tmp/path] [--bundle tmp/path.zip]';

async function main() {
  if (process.argv.includes('--help') || process.argv.includes('-h')) {
    console.log(usage);
    return;
  }
  const options = {
    architecture: 'x64',
    descriptorPath: valueAfter('--release'),
    descriptorSha256: valueAfter('--release-sha256'),
    provenancePath: valueAfter('--provenance'),
    provenanceSha256: valueAfter('--provenance-sha256'),
    sourceRoot: valueAfter('--source'),
    requestPrivateKeyPath: valueAfter('--request-private-key'),
    signerPath: valueAfter('--signer'),
    signerSha256: valueAfter('--signer-sha256'),
    manifestPrivateKeyPath: valueAfter('--manifest-private-key'),
    updatePrivateKeyPath: valueAfter('--update-private-key'),
    validationPrivateKeyPath: valueAfter('--validation-private-key'),
    notBeforeMs: valueAfter('--not-before-ms'),
    expiresAtMs: valueAfter('--expires-at-ms'),
    buildId: valueAfter('--build-id'),
    outputRoot: valueAfter('--output'),
    kitOutputRoot: valueAfter('--kit-output'),
    bundlePath: valueAfter('--bundle'),
  };
  if (
    [
      options.descriptorPath,
      options.descriptorSha256,
      options.provenancePath,
      options.provenanceSha256,
      options.sourceRoot,
      options.requestPrivateKeyPath,
      options.signerPath,
      options.signerSha256,
      options.manifestPrivateKeyPath,
      options.updatePrivateKeyPath,
      options.validationPrivateKeyPath,
      options.notBeforeMs,
      options.expiresAtMs,
    ].some((value) => !value)
  ) {
    throw new Error('Run build-windows-installed-acceptance-inputs.mjs --help for usage.');
  }
  const result = await buildWindowsInstalledAcceptanceInputs(options);
  console.log(canonicalAcceptanceJson(result));
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
