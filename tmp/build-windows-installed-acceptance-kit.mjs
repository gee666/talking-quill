import { spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, cp, lstat, mkdir, readFile, readdir, rm, writeFile } from 'node:fs/promises';
import { basename, dirname, relative, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseTqpkg2 } from '../scripts/tqpkg2.mjs';
import { verifyAcceptancePreflight } from '../scripts/windows-acceptance-preflight.mjs';
import { canonicalAcceptanceJson } from '../scripts/windows-installed-acceptance-probe.mjs';
import {
  ACCEPTANCE_FAULT_PHASES,
  ACCEPTANCE_REQUEST_SCHEDULE,
  MAX_ACCEPTANCE_REQUEST_MS,
  createInstalledAcceptancePlan,
  validateAcceptanceRunSequence,
} from '../scripts/windows-installed-acceptance.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
const signer = resolve(root, 'scripts/windows-installed-acceptance-signer.mjs');
const PRIVATE_ENVIRONMENT = /(?:PRIVATE_KEY|SIGNING_KEY|REQUEST_PRIVATE)/u;

export function sanitizedBuildEnvironment(environment = process.env) {
  return Object.fromEntries(
    Object.entries(environment).filter(([name]) => !PRIVATE_ENVIRONMENT.test(name)),
  );
}

export async function validateCanonicalRelease({ descriptorPath, descriptorSha256, sourceRoot }) {
  const descriptorBytes = await readRegular(descriptorPath);
  if (sha256(descriptorBytes) !== descriptorSha256) {
    throw new Error('Canonical RELEASE descriptor SHA-256 does not match');
  }
  const descriptor = JSON.parse(descriptorBytes.toString('utf8'));
  if (
    descriptor.schemaVersion !== 1 ||
    descriptor.result !== 'passed' ||
    descriptor.version !== '0.0.69' ||
    descriptor.packageMode !== 'fresh' ||
    descriptor.variant !== 'canonical' ||
    !['x64', 'arm64'].includes(descriptor.architecture) ||
    !/^[0-9a-f]{40}$/u.test(descriptor.sourceCommit ?? '') ||
    !/^[0-9a-f]{40}$/u.test(descriptor.sourceTree ?? '') ||
    !/^[0-9a-f]{64}$/u.test(descriptor.sha256 ?? '') ||
    !Number.isSafeInteger(descriptor.bytes) ||
    descriptor.bytes <= 0 ||
    basename(descriptor.installer ?? '') !== descriptor.installer
  ) {
    throw new Error('Canonical RELEASE descriptor is invalid');
  }
  const installerPath = resolve(dirname(resolve(descriptorPath)), descriptor.installer);
  const installerBytes = await readRegular(installerPath);
  if (installerBytes.length !== descriptor.bytes || sha256(installerBytes) !== descriptor.sha256) {
    throw new Error('Canonical installer does not match RELEASE');
  }
  const parsed = parseTqpkg2(installerBytes, descriptor.architecture);
  if (
    parsed.manifest.packageMode !== 'fresh' ||
    parsed.manifest.version !== descriptor.version ||
    parsed.manifest.sourceCommit !== descriptor.sourceCommit ||
    parsed.manifest.sourceTree !== descriptor.sourceTree ||
    parsed.manifest.treeSha256 !== descriptor.tqpkg2TreeSha256
  ) {
    throw new Error('Canonical RELEASE does not bind the complete TQPKG2 package');
  }
  const commit = git(sourceRoot, ['rev-parse', `${descriptor.sourceCommit}^{commit}`]);
  const tree = git(sourceRoot, ['rev-parse', `${descriptor.sourceCommit}^{tree}`]);
  if (commit !== descriptor.sourceCommit || tree !== descriptor.sourceTree) {
    throw new Error('Canonical RELEASE source commit/tree is unavailable or mismatched');
  }
  return Object.freeze({ descriptor, descriptorBytes, installerPath, installerBytes });
}

export async function buildInstalledAcceptanceKit(options) {
  const imported = await validateCanonicalRelease(options);
  const configBytes = await readRegular(options.configPath);
  const config = JSON.parse(configBytes.toString('utf8'));
  if (config.architecture !== imported.descriptor.architecture) {
    throw new Error('Acceptance kit architecture differs from canonical RELEASE');
  }
  for (const name of ['predecessor', 'fresh']) {
    if (config.artifacts?.[name]?.installerSha256 !== imported.descriptor.sha256) {
      throw new Error(`${name} must import the exact canonical installer bytes`);
    }
  }
  const faultNames = Object.keys(config.artifacts?.faults ?? {});
  if (canonicalAcceptanceJson(faultNames) !== canonicalAcceptanceJson(ACCEPTANCE_FAULT_PHASES)) {
    throw new Error('Acceptance kit must carry ten ordered, isolated fault packages');
  }
  const outputRoot = resolve(options.outputRoot ?? 'tmp/windows-installed-acceptance/kit');
  if (!isBelow(resolve(root, 'tmp'), outputRoot)) {
    throw new Error('Acceptance kit output must stay under tmp');
  }
  await rm(outputRoot, { recursive: true, force: true });
  await mkdir(resolve(outputRoot, 'imported'), { recursive: true });
  const stagedConfig = structuredClone(config);
  for (const name of ['predecessor', 'candidate', 'fresh', 'repair', 'fault']) {
    stagedConfig.artifacts[name] = await stageArtifact(name, config.artifacts[name], outputRoot);
  }
  stagedConfig.artifacts.faults = {};
  for (const phase of ACCEPTANCE_FAULT_PHASES) {
    stagedConfig.artifacts.faults[phase] = await stageArtifact(
      `fault-${phase}`,
      config.artifacts.faults[phase],
      outputRoot,
    );
  }
  const candidateInput = config.artifacts.candidate;
  const candidateOutput = stagedConfig.artifacts.candidate;
  const embeddedManifestRelative = relative(
    resolve(candidateInput.unpackedRoot),
    resolve(config.acceptance.buildManifestPath),
  );
  if (embeddedManifestRelative.startsWith('..') || embeddedManifestRelative.includes(':')) {
    throw new Error('Acceptance build manifest is outside the candidate unpacked tree');
  }
  stagedConfig.acceptance.buildManifestPath = resolve(
    candidateOutput.unpackedRoot,
    embeddedManifestRelative,
  );
  for (const [field, fileName] of [
    ['syntheticSenderPath', 'synthetic-sender.exe'],
    ['trustedLauncherPath', 'trusted-launcher.exe'],
  ]) {
    const destination = resolve(outputRoot, 'acceptance', fileName);
    await mkdir(dirname(destination), { recursive: true });
    await copyFile(config.acceptance[field], destination);
    stagedConfig.acceptance[field] = destination;
  }
  const requestKeyBytes = await readRegular(options.requestPrivateKeyPath);
  const signedRequests = {};
  for (const invocation of ACCEPTANCE_REQUEST_SCHEDULE) {
    const supplied = config.acceptance?.requestPayloads?.[invocation.invocationId];
    if (supplied === null || typeof supplied !== 'object' || Array.isArray(supplied)) {
      throw new Error(`Acceptance request payload is missing: ${invocation.invocationId}`);
    }
    const deadline = config.acceptance.runWindow.notBeforeMs + invocation.deadlineOffsetMs;
    const payload = {
      ...supplied,
      version: 1,
      purpose: 'talking-quill/installed-acceptance-run',
      command: invocation.command,
      buildId: config.acceptance.buildId,
      invocationId: invocation.invocationId,
      latestStartOffsetMs: invocation.latestStartOffsetMs,
      deadlineOffsetMs: invocation.deadlineOffsetMs,
      runWindow: config.acceptance.runWindow,
      issuedAtMs: deadline - MAX_ACCEPTANCE_REQUEST_MS,
      expiresAtMs: deadline,
    };
    const encoded = signInNarrowSubprocess(payload, requestKeyBytes);
    const current = signedRequests[invocation.command];
    if (current === undefined) signedRequests[invocation.command] = encoded;
    else if (Array.isArray(current)) current.push(encoded);
    else signedRequests[invocation.command] = [current, encoded];
  }
  const evidenceInput = {
    ...stagedConfig,
    canonicalRelease: {
      descriptorSha256: sha256(imported.descriptorBytes),
      installerSha256: imported.descriptor.sha256,
      sourceCommit: imported.descriptor.sourceCommit,
      sourceTree: imported.descriptor.sourceTree,
    },
    acceptance: {
      ...stagedConfig.acceptance,
      signedRequestsPath: '',
      signedRequestsSha256: '',
    },
  };
  delete evidenceInput.acceptance.requestPayloads;
  const requestsBytes = Buffer.from(`${canonicalAcceptanceJson(signedRequests)}\n`);
  const requestsPath = resolve(outputRoot, 'signed-requests.json');
  await writeFile(requestsPath, requestsBytes, { mode: 0o600 });
  evidenceInput.acceptance.signedRequestsPath = requestsPath;
  evidenceInput.acceptance.signedRequestsSha256 = sha256(requestsBytes);
  const portableEvidenceInput = portablePaths(evidenceInput, outputRoot);
  const evidenceBytes = Buffer.from(`${canonicalAcceptanceJson(portableEvidenceInput)}\n`);
  const evidencePath = resolve(outputRoot, 'evidence-input.json');
  await writeFile(evidencePath, evidenceBytes, { mode: 0o600 });
  const releaseCopy = resolve(outputRoot, 'imported', 'RELEASE.json');
  const installerCopy = resolve(outputRoot, 'imported', imported.descriptor.installer);
  await copyFile(options.descriptorPath, releaseCopy);
  await copyFile(imported.installerPath, installerCopy);
  const before = sha256(imported.installerBytes);
  if (
    sha256(await readFile(imported.installerPath)) !== before ||
    sha256(await readFile(installerCopy)) !== before
  ) {
    throw new Error('Canonical installer bytes changed while assembling the kit');
  }
  const plan = await createInstalledAcceptancePlan(evidenceInput);
  const sequence = validateAcceptanceRunSequence(
    plan.acceptance,
    plan.acceptance.runWindow.notBeforeMs,
  );
  await verifyAcceptancePreflight({
    plan,
    sequence,
    nowMs: plan.acceptance.runWindow.notBeforeMs,
    reserveNonces: false,
  });
  const inventoryPaths = await collectRegularFiles(outputRoot);
  const entries = await Promise.all(
    inventoryPaths.map(async (path) => {
      const bytes = await readFile(path);
      return {
        path: relative(outputRoot, path).split(sep).join('/'),
        bytes: bytes.length,
        sha256: sha256(bytes),
      };
    }),
  );
  entries.sort((left, right) => left.path.localeCompare(right.path, 'en'));
  const manifest = {
    schemaVersion: 1,
    classification: 'nonpromotable-installed-acceptance-kit',
    sourceCommit: imported.descriptor.sourceCommit,
    sourceTree: imported.descriptor.sourceTree,
    entries,
  };
  await writeFile(
    resolve(outputRoot, 'bundle-manifest.json'),
    `${canonicalAcceptanceJson(manifest)}\n`,
    { mode: 0o600 },
  );
  return Object.freeze({ outputRoot, evidencePath, manifest });
}

function signInNarrowSubprocess(payload, keyBytes) {
  const input = canonicalAcceptanceJson({
    operation: 'acceptance-envelope',
    payload,
    privateKeyPkcs8Base64: keyBytes.toString('base64'),
  });
  const result = spawnSync(process.execPath, [signer], {
    cwd: root,
    env: Object.fromEntries(
      Object.entries({ SystemRoot: process.env.SystemRoot, WINDIR: process.env.WINDIR }).filter(
        ([, value]) => value !== undefined,
      ),
    ),
    input,
    encoding: 'utf8',
    windowsHide: true,
    timeout: 10_000,
    maxBuffer: 64 * 1024,
  });
  if (result.status !== 0) throw new Error('Narrow acceptance signing subprocess failed');
  const signed = JSON.parse(result.stdout);
  if (typeof signed.encoded !== 'string') throw new Error('Narrow signer returned invalid output');
  return signed.encoded;
}

async function stageArtifact(name, input, outputRoot) {
  if (input === null || typeof input !== 'object' || Array.isArray(input)) {
    throw new Error(`Kit artifact is missing: ${name}`);
  }
  const artifactRoot = resolve(outputRoot, 'artifacts', name);
  const unpackedRoot = resolve(artifactRoot, 'unpacked');
  await mkdir(artifactRoot, { recursive: true });
  await cp(resolve(input.unpackedRoot), unpackedRoot, {
    recursive: true,
    force: false,
    errorOnExist: true,
    dereference: false,
    preserveTimestamps: false,
  });
  const output = { ...input, unpackedRoot };
  for (const [field, fileName] of [
    ['installerPath', 'installer.exe'],
    ['metadataPath', 'metadata.json'],
    ['releaseIdentityPath', 'release-identity.json'],
    ['validationEvidencePath', 'isolated-validation.json'],
  ]) {
    if (input[field] === undefined) continue;
    const destination = resolve(artifactRoot, fileName);
    await copyFile(input[field], destination);
    output[field] = destination;
  }
  for (const field of ['electronPath', 'appAsarPath']) {
    if (input[field] === undefined) continue;
    const local = relative(resolve(input.unpackedRoot), resolve(input[field]));
    if (local.startsWith('..') || local.includes(':')) {
      throw new Error(`${name} ${field} is outside its unpacked tree`);
    }
    output[field] = resolve(unpackedRoot, local);
  }
  return output;
}

function portablePaths(input, outputRoot) {
  const portable = structuredClone(input);
  const fields = [
    'installerPath',
    'metadataPath',
    'releaseIdentityPath',
    'validationEvidencePath',
    'unpackedRoot',
    'electronPath',
    'appAsarPath',
  ];
  const artifacts = [
    portable.artifacts.predecessor,
    portable.artifacts.candidate,
    portable.artifacts.fresh,
    portable.artifacts.repair,
    portable.artifacts.fault,
    ...Object.values(portable.artifacts.faults),
  ];
  for (const artifact of artifacts) {
    for (const field of fields) {
      if (artifact[field] !== undefined)
        artifact[field] = portablePath(artifact[field], outputRoot);
    }
  }
  for (const field of [
    'buildManifestPath',
    'signedRequestsPath',
    'syntheticSenderPath',
    'trustedLauncherPath',
  ]) {
    portable.acceptance[field] = portablePath(portable.acceptance[field], outputRoot);
  }
  return portable;
}

function portablePath(path, outputRoot) {
  const local = relative(outputRoot, resolve(path));
  if (local === '' || local === '..' || local.startsWith(`..${sep}`) || local.includes(':')) {
    throw new Error('Kit path escapes the deterministic bundle');
  }
  return local.split(sep).join('/');
}

async function collectRegularFiles(directory) {
  const paths = [];
  const visit = async (current) => {
    for (const entry of await readdir(current, { withFileTypes: true })) {
      const path = resolve(current, entry.name);
      const metadata = await lstat(path);
      if (metadata.isSymbolicLink()) throw new Error('Kit bundle contains a link');
      if (metadata.isDirectory()) await visit(path);
      else if (metadata.isFile()) paths.push(path);
      else throw new Error('Kit bundle contains a non-regular entry');
    }
  };
  await visit(directory);
  return paths;
}

async function readRegular(path) {
  const absolute = resolve(path);
  const metadata = await lstat(absolute);
  if (!metadata.isFile() || metadata.isSymbolicLink()) {
    throw new Error(`Kit input is not a regular file: ${basename(absolute)}`);
  }
  return readFile(absolute);
}

function git(sourceRoot, arguments_) {
  const result = spawnSync('git', ['-C', resolve(sourceRoot), ...arguments_], {
    encoding: 'utf8',
    windowsHide: true,
    timeout: 10_000,
  });
  if (result.status !== 0) throw new Error('Canonical source identity lookup failed');
  return result.stdout.trim();
}

function isBelow(parent, child) {
  const value = relative(parent, child);
  return value !== '' && value !== '..' && !value.startsWith(`..${sep}`) && !value.includes(':');
}

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}

async function main() {
  const options = {
    descriptorPath: valueAfter('--release'),
    descriptorSha256: valueAfter('--release-sha256'),
    sourceRoot: valueAfter('--source'),
    configPath: valueAfter('--config'),
    requestPrivateKeyPath: valueAfter('--request-private-key'),
    outputRoot: valueAfter('--output'),
  };
  if (
    Object.entries(options)
      .slice(0, 5)
      .some(([, value]) => !value)
  ) {
    throw new Error(
      'Usage: node tmp/build-windows-installed-acceptance-kit.mjs --release RELEASE.json --release-sha256 <sha256> --source <git-root> --config <kit-input.json> --request-private-key <P-256-pkcs8-der> [--output tmp/path]',
    );
  }
  const result = await buildInstalledAcceptanceKit(options);
  console.log(canonicalAcceptanceJson(result));
}

if (resolve(process.argv[1] ?? '') === fileURLToPath(import.meta.url)) await main();
