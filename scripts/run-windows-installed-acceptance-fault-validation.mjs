import { spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { lstat, mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { hostname, userInfo } from 'node:os';
import { resolve } from 'node:path';
import { parseTqpkg2 } from './tqpkg2.mjs';
import {
  encodeSignedFaultEvidence,
  faultEvidenceGenesis,
} from './windows-installed-acceptance-fault-evidence.mjs';
import { signAcceptancePayload } from './windows-installed-acceptance-signer.mjs';
import { ACCEPTANCE_FAULT_PHASES } from './windows-installed-acceptance-schedule.mjs';
import { canonicalAcceptanceJson } from './windows-installed-acceptance-probe.mjs';
import { sanitizedSubprocessEnvironment } from './environment-policy.mjs';

if (process.platform !== 'win32') throw new Error('Fault validation requires Windows');
const phase = valueAfter('--phase');
const sequence = ACCEPTANCE_FAULT_PHASES.indexOf(phase);
if (sequence < 0) throw new Error('Fault validation phase is invalid');
const outputRoot = resolve(valueAfter('--output') ?? '');
const secrets = await readFrame();
const root = resolve(import.meta.dirname, '..');
const packageRoot = resolve(root, 'tmp/installed-acceptance-build');
const faultPath = resolve(packageRoot, `Talking-Quill-0.0.69-win-x64-repair-${phase}.exe`);
const repairPath = resolve(packageRoot, 'Talking-Quill-0.0.69-win-x64-repair.exe');
const candidatePath = resolve(packageRoot, 'Talking-Quill-0.0.69-win-x64-update.exe');
const [faultBytes, candidateBytes] = await Promise.all([
  readFile(faultPath),
  readFile(candidatePath),
]);
const parsedFault = parseTqpkg2(faultBytes, 'x64', {
  allowAcceptanceFaults: true,
  allowAcceptanceRepair: true,
});
if (parsedFault.manifest.faultPhase !== phase) {
  throw new Error('Fault package does not contain the requested exact phase');
}
const testMarker = Buffer.from('TQ_MACHINE_LOCK_TEST_NAMESPACE_ID', 'ascii');
if (!faultBytes.includes(testMarker)) {
  throw new Error('Fault package lacks the isolated machine-lock namespace build gate');
}
const candidateMetadata = JSON.parse(
  await readFile(
    resolve(packageRoot, 'win-unpacked/resources/keyboard-owner-release-v1.json'),
    'utf8',
  ),
);
const namespaceId = randomBytes(16).toString('hex');
const namespaceRoot = resolve(root, 'tmp/machine-lock-tests/windows-setup', namespaceId);
await mkdir(namespaceRoot, { recursive: true });
const productionBefore = await productionState();
const isolatedBefore = await treeHash(namespaceRoot);
const environment = sanitizedSubprocessEnvironment(process.env, {
  TQ_MACHINE_LOCK_TEST_NAMESPACE_ID: namespaceId,
  TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD: '1',
});
const fault = run(faultPath, environment);
if (fault.status !== 197) throw new Error(`Fault package did not stop at ${phase}`);
const recovery = run(repairPath, environment);
if (recovery.status !== 0) throw new Error(`Repair recovery failed after ${phase}`);
const isolatedAfter = await treeHash(namespaceRoot);
if (isolatedBefore !== isolatedAfter) {
  throw new Error(`Repair did not restore isolated state after ${phase}`);
}
await rm(namespaceRoot, { recursive: true, force: false });
const residue = await stat(namespaceRoot).then(
  () => false,
  (error) => error?.code === 'ENOENT',
);
const productionAfter = await productionState();
if (productionBefore !== productionAfter || !residue) {
  throw new Error(`Isolated fault validation left production or namespace residue: ${phase}`);
}
const priorPath =
  sequence === 0
    ? null
    : resolve(outputRoot, `fault-validation-${ACCEPTANCE_FAULT_PHASES[sequence - 1]}.json`);
const previousEnvelopeSha256 =
  priorPath === null
    ? faultEvidenceGenesis(process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID, hash(candidateBytes))
    : hash(await readFile(priorPath));
const signerPath = resolve(outputRoot, 'native/talking-quill-acceptance-signer.exe');
const brokerPath = resolve(outputRoot, 'native/talking-quill-windows-acceptance-broker.exe');
const payload = {
  schemaVersion: 1,
  purpose: 'talking-quill/installed-acceptance-fault-validation',
  architecture: 'x64',
  buildId: process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID,
  sourceCommit: process.env.TALKING_QUILL_RELEASE_COMMIT,
  sourceTree: process.env.TALKING_QUILL_RELEASE_TREE,
  sequence,
  faultPhase: phase,
  previousEnvelopeSha256,
  candidatePackageSha256: hash(candidateBytes),
  candidatePackageLayoutDigest: candidateMetadata.packageLayoutDigest,
  faultPackageSha256: hash(faultBytes),
  faultPackageTreeSha256: parsedFault.manifest.treeSha256,
  validatorSha256: await hashFile(new URL(import.meta.url)),
  namespaceIdSha256: hash(Buffer.from(namespaceId)),
  machineIdentitySha256: hash(Buffer.from(hostname())),
  sessionIdentitySha256: hash(
    Buffer.from(`${userInfo().username}\0${process.env.SESSIONNAME ?? ''}`),
  ),
  productionStateBeforeSha256: productionBefore,
  productionStateAfterSha256: productionAfter,
  isolatedStateBeforeSha256: isolatedBefore,
  isolatedStateAfterSha256: isolatedAfter,
  faultExitCode: fault.status,
  recoveryExitCode: recovery.status,
  failurePointObserved: phase,
  failureInjected: true,
  recoveryCompleted: true,
  zeroResidue: true,
  productionStateUnchanged: true,
  mixedAuthorityAbsent: true,
};
const signed = signAcceptancePayload({
  signerPath,
  signerSha256: await hashFile(signerPath),
  brokerPath,
  brokerSha256: await hashFile(brokerPath),
  signerSourceCommit: payload.sourceCommit,
  signerSourceTree: payload.sourceTree,
  privateKeyPath: secrets.validationPrivateKeyPath,
  payloadBytes: Buffer.from(canonicalAcceptanceJson(payload)),
});
const evidence = encodeSignedFaultEvidence(payload, signed);
await writeFile(resolve(outputRoot, `fault-validation-${phase}.json`), evidence, {
  flag: 'wx',
  mode: 0o600,
});
console.log(canonicalAcceptanceJson({ result: 'passed', phase, sha256: hash(evidence) }));

function run(executable, env) {
  return spawnSync(executable, ['/S'], {
    cwd: root,
    env,
    windowsHide: true,
    stdio: 'ignore',
    timeout: 180_000,
  });
}

async function productionState() {
  const roots = [
    resolve(process.env.ProgramW6432 ?? 'C:/Program Files', 'Talking Quill'),
    resolve(process.env.ProgramData ?? 'C:/ProgramData', 'Talking Quill'),
  ];
  const states = [];
  for (const path of roots) states.push(await treeHash(path));
  return hash(Buffer.from(canonicalAcceptanceJson(states)));
}

async function treeHash(path) {
  const hashState = createHash('sha256');
  const visit = async (current, local) => {
    const entries = await readdir(current, { withFileTypes: true }).catch((error) => {
      if (error?.code === 'ENOENT') return [];
      throw error;
    });
    entries.sort((left, right) => left.name.localeCompare(right.name));
    for (const entry of entries) {
      const next = resolve(current, entry.name);
      const name = `${local}/${entry.name}`;
      const metadata = await lstat(next);
      if (metadata.isSymbolicLink()) throw new Error('Validation state contains a link');
      hashState.update(name).update(String(metadata.size));
      if (metadata.isDirectory()) await visit(next, name);
      else if (metadata.isFile()) hashState.update(await readFile(next));
      else throw new Error('Validation state contains a special file');
    }
  };
  await visit(path, '');
  return hashState.digest('hex');
}

async function hashFile(path) {
  return hash(await readFile(path));
}
function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
async function readFrame() {
  const bytes = await readFile(0);
  if (bytes.length === 0 || bytes.length > 16 * 1024 || bytes.at(-1) !== 0x0a) {
    throw new Error('Fault validator secret-path frame is invalid');
  }
  return JSON.parse(bytes.subarray(0, -1).toString('utf8'));
}
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
