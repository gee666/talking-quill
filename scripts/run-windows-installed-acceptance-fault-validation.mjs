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
import { readAcceptanceSecretPaths } from './acceptance-secret-path-frame.mjs';

if (process.platform !== 'win32') throw new Error('Fault validation requires Windows');
const phase = valueAfter('--phase');
const sequence = ACCEPTANCE_FAULT_PHASES.indexOf(phase);
if (sequence < 0) throw new Error('Fault validation phase is invalid');
const outputRoot = resolve(valueAfter('--output') ?? '');
const secrets = readAcceptanceSecretPaths(
  undefined,
  'Fault validator secret-path frame is invalid',
);
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
const auditNonce = randomBytes(32).toString('hex');
const before = await measureState(namespaceRoot, namespaceId);
const environment = sanitizedSubprocessEnvironment(process.env, {
  TQ_MACHINE_LOCK_TEST_NAMESPACE_ID: namespaceId,
  TQ_FAULT_AUDIT_NONCE: auditNonce,
  TALKING_QUILL_WINDOWS_INSTALLED_ACCEPTANCE_BUILD: '1',
});
const fault = run(faultPath, environment);
if (fault.status !== 197) throw new Error(`Fault package did not stop at ${phase}`);
const faulted = await measureState(namespaceRoot, namespaceId);
const auditPath = resolve(namespaceRoot, 'fault-audit-v1.json');
const auditBytes = await readFile(auditPath);
const faultAudit = JSON.parse(auditBytes.toString('utf8'));
if (
  auditBytes.toString('utf8') !== `${canonicalAcceptanceJson(faultAudit)}\n` ||
  faultAudit.phase !== phase ||
  faultAudit.nonce !== auditNonce ||
  !Number.isSafeInteger(faultAudit.processId) ||
  faultAudit.processId <= 0
) {
  throw new Error(`Fault worker audit did not prove the requested phase: ${phase}`);
}
await rm(auditPath, { force: false });
const recovery = run(repairPath, environment);
if (recovery.status !== 0) throw new Error(`Repair recovery failed after ${phase}`);
const recovered = await measureState(namespaceRoot, namespaceId);
if (
  before.namespaceTreeSha256 !== recovered.namespaceTreeSha256 ||
  canonicalAcceptanceJson(before.production) !== canonicalAcceptanceJson(recovered.production) ||
  recovered.namespace.registry.length !== 0 ||
  recovered.namespace.processes.length !== 0 ||
  recovered.namespace.services.length !== 0 ||
  recovered.namespace.tasks.length !== 0 ||
  recovered.namespace.journals.length !== 0 ||
  recovered.namespace.heldMutexes.length !== 0
) {
  throw new Error(`Measured recovery retained mixed authority or namespace state: ${phase}`);
}
await rm(namespaceRoot, { recursive: true, force: false });
const residue = await stat(namespaceRoot).then(
  () => false,
  (error) => error?.code === 'ENOENT',
);
if (!residue) throw new Error(`Fault namespace could not be retired: ${phase}`);
const priorPath =
  sequence === 0
    ? null
    : resolve(outputRoot, `fault-validation-${ACCEPTANCE_FAULT_PHASES[sequence - 1]}.json`);
const previousEnvelopeSha256 =
  priorPath === null
    ? faultEvidenceGenesis(process.env.TALKING_QUILL_ACCEPTANCE_BUILD_ID, hash(candidateBytes))
    : hash(await readFile(priorPath));
const signingIdentities = JSON.parse(
  await readFile(resolve(outputRoot, 'signing-identities.json'), 'utf8'),
);
const signerPath = resolve(signingIdentities.signerPath);
const brokerPath = resolve(signingIdentities.acceptanceBrokerPath);
const bootstrapPath = resolve(signingIdentities.acceptanceBootstrapPath);
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
  before,
  faulted,
  recovered,
  faultAudit,
  faultAuditSha256: hash(auditBytes),
  faultExitCode: fault.status,
  recoveryExitCode: recovery.status,
};
const signed = signAcceptancePayload({
  signerPath,
  signerSha256: await hashFile(signerPath),
  signerBytes: (await lstat(signerPath)).size,
  brokerPath,
  brokerSha256: await hashFile(brokerPath),
  brokerBytes: (await lstat(brokerPath)).size,
  bootstrapIdentity: {
    path: bootstrapPath,
    sha256: await hashFile(bootstrapPath),
    bytes: (await lstat(bootstrapPath)).size,
  },
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

async function measureState(namespaceRoot, namespaceId) {
  const programFiles = resolve(process.env.ProgramW6432 ?? 'C:/Program Files', 'Talking Quill');
  const programData = resolve(process.env.ProgramData ?? 'C:/ProgramData');
  const script = String.raw`
$ErrorActionPreference='Stop'
$id='${namespaceId}'
function Registry($path) {
  if (-not (Test-Path -LiteralPath $path)) { return @() }
  $keys=@(Get-Item -LiteralPath $path)+(Get-ChildItem -LiteralPath $path -Recurse -Force)
  @($keys | ForEach-Object {
    $key=$_
    "KEY|$($key.PSPath)"
    $properties=Get-ItemProperty -LiteralPath $key.PSPath
    $properties.PSObject.Properties | Where-Object { $_.Name -notlike 'PS*' } | ForEach-Object {
      "VALUE|$($key.PSPath)|$($_.Name)|$([Convert]::ToHexString([Text.Encoding]::UTF8.GetBytes([string]$_.Value)).ToLowerInvariant())"
    }
  })
}
$registry=@(Registry "HKCU:\Software\Talking Quill Tests\$id")
$productionRegistry=@(
  Registry 'HKLM:\Software\Talking Quill';
  Registry 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill';
  Registry 'HKLM:\Software\Microsoft\Windows\CurrentVersion\App Paths\Talking Quill.exe';
  Registry 'HKLM:\Software\Microsoft\Windows\CurrentVersion\Run';
  Registry 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Talking Quill';
  Registry 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
)
$processes=@(Get-CimInstance Win32_Process | Where-Object { $_.ProcessId -ne $PID -and (($_.ExecutablePath -like '*Talking Quill*') -or ($_.CommandLine -like "*$id*")) } | ForEach-Object { $digest=if(Test-Path -LiteralPath $_.ExecutablePath){(Get-FileHash -LiteralPath $_.ExecutablePath -Algorithm SHA256).Hash.ToLowerInvariant()}else{'missing'}; "{0}:{1}:{2}:{3}" -f $_.ProcessId,$_.ParentProcessId,$_.ExecutablePath,$digest })
$services=@(Get-CimInstance Win32_Service | Where-Object { $_.Name -like 'TalkingQuill*' } | ForEach-Object { "{0}:{1}:{2}:{3}" -f $_.Name,$_.State,$_.StartMode,$_.PathName })
$tasks=@(& "$env:WINDIR\System32\schtasks.exe" /Query /V /FO CSV /NH 2>$null | Where-Object { $_ -like '*TalkingQuill*' })
$held=@()
foreach($name in @("Local\TalkingQuill.Tests.$id.NativeSetup.V2","Local\TalkingQuill.Tests.$id.UpdateRecovery.State.V1")){try{$m=[Threading.Mutex]::OpenExisting($name);if(-not $m.WaitOne(0)){$held+=$name}else{$m.ReleaseMutex()};$m.Dispose()}catch[Threading.WaitHandleCannotBeOpenedException]{}}
[ordered]@{registry=@($registry|Sort-Object);productionRegistry=@($productionRegistry|Sort-Object);processes=@($processes|Sort-Object);services=@($services|Sort-Object);tasks=@($tasks|Sort-Object);heldMutexes=@($held|Sort-Object)} | ConvertTo-Json -Depth 6 -Compress`;
  const powershell = spawnSync(
    resolve(
      process.env.SystemRoot ?? 'C:/Windows',
      'System32/WindowsPowerShell/v1.0/powershell.exe',
    ),
    ['-NoProfile', '-NonInteractive', '-Command', script],
    { encoding: 'utf8', windowsHide: true, timeout: 30_000 },
  );
  if (powershell.status !== 0 || powershell.stderr !== '') {
    throw new Error('Native fault state inspection failed');
  }
  const observed = JSON.parse(powershell.stdout);
  const namespaceFiles = await fileInventory(namespaceRoot);
  const productionFiles = {
    programFiles: await fileInventory(programFiles),
    programData: await fileInventory(resolve(programData, 'Talking Quill')),
    recovery: await fileInventory(resolve(programData, 'Talking Quill Update Recovery')),
  };
  return {
    schemaVersion: 1,
    namespaceTreeSha256: await treeHash(namespaceRoot),
    namespace: {
      files: namespaceFiles,
      journals: namespaceFiles.filter((entry) => /journal|transaction|recovery/iu.test(entry.path)),
      registry: observed.registry,
      processes: observed.processes.filter((entry) => entry.includes(namespaceId)),
      services: observed.services.filter((entry) => entry.includes(namespaceId)),
      tasks: observed.tasks.filter((entry) => entry.includes(namespaceId)),
      heldMutexes: observed.heldMutexes,
    },
    production: {
      files: productionFiles,
      registry: observed.productionRegistry,
      processes: observed.processes.filter((entry) => !entry.includes(namespaceId)),
      services: observed.services.filter((entry) => !entry.includes(namespaceId)),
      tasks: observed.tasks.filter((entry) => !entry.includes(namespaceId)),
    },
  };
}

async function fileInventory(path) {
  const entries = [];
  const visit = async (current, local) => {
    const children = await readdir(current, { withFileTypes: true }).catch((error) => {
      if (error?.code === 'ENOENT') return [];
      throw error;
    });
    children.sort((left, right) => left.name.localeCompare(right.name));
    for (const child of children) {
      const next = resolve(current, child.name);
      const relative = `${local}/${child.name}`;
      const metadata = await lstat(next);
      if (metadata.isSymbolicLink()) throw new Error('Validation state contains a link');
      if (metadata.isDirectory()) await visit(next, relative);
      else if (metadata.isFile())
        entries.push({ path: relative, bytes: metadata.size, sha256: hash(await readFile(next)) });
      else throw new Error('Validation state contains a special file');
    }
  };
  await visit(path, '');
  return entries;
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
function valueAfter(name) {
  const index = process.argv.indexOf(name);
  return index < 0 ? undefined : process.argv[index + 1];
}
