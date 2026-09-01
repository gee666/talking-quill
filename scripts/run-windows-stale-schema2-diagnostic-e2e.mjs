import { spawn, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { copyFile, mkdir, readFile, rm, writeFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseTqpkg2 } from './tqpkg2.mjs';

const root = resolve(fileURLToPath(new URL('..', import.meta.url)));
if (process.platform !== 'win32') {
  throw new Error('The stale schema-2 packaged executable test requires Windows');
}
const productionNamespaceLock = await acquireProductionNamespaceLock();
process.on('exit', () => productionNamespaceLock.stdin.end('\n'));
const architecture = process.argv[2] ?? 'x64';
const expectedTopology = process.env.TQ_STALE_SCHEMA2_EXPECT_TOPOLOGY ?? 'exact-orphan-lock-only';
if (!['exact-orphan-lock-only', 'exact-schema2-fixture'].includes(expectedTopology)) {
  throw new Error('TQ_STALE_SCHEMA2_EXPECT_TOPOLOGY is invalid');
}
if (!['x64', 'arm64'].includes(architecture)) {
  throw new Error('Usage: run-windows-stale-schema2-diagnostic-e2e.mjs [x64|arm64]');
}
const sourceCommit = git(['rev-parse', 'HEAD']);
const sourceTree = git(['rev-parse', 'HEAD^{tree}']);
if (!/^[0-9a-f]{40}$/u.test(sourceCommit) || !/^[0-9a-f]{40}$/u.test(sourceTree)) {
  throw new Error('packaged diagnostic test requires exact current Git source identity');
}
if (git(['status', '--porcelain', '--untracked-files=no']) !== '') {
  throw new Error('packaged diagnostic test requires a clean tracked worktree');
}
const version = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8')).version;
const canonical = resolve(
  root,
  'release',
  `Talking-Quill-${version}-win-${architecture}-setup.exe`,
);
const packagedDiagnostic = resolve(
  root,
  'release',
  `Talking-Quill-${version}-win-${architecture}-stale-schema2-cleanup.exe`,
);
await rebuildCurrentArtifacts();
const canonicalPackage = parseTqpkg2(await readFile(canonical), architecture);
if (
  canonicalPackage.manifest.packageMode !== 'fresh' ||
  canonicalPackage.manifest.sourceCommit !== sourceCommit ||
  canonicalPackage.manifest.sourceTree !== sourceTree
) {
  throw new Error('canonical packaged executable is not bound to the current Git source');
}
const diagnosticPackage = parseTqpkg2(await readFile(packagedDiagnostic), architecture, {
  allowStaleSchema2Cleanup: true,
});
if (
  diagnosticPackage.manifest.packageMode !== 'stale-schema2-cleanup' ||
  diagnosticPackage.manifest.sourceCommit !== sourceCommit ||
  diagnosticPackage.manifest.sourceTree !== sourceTree
) {
  throw new Error('diagnostic packaged executable is not bound to the current Git source');
}
const testRoot = resolve(root, 'tmp', 'stale-schema2-packaged-diagnostic-e2e');
await rm(testRoot, { recursive: true, force: true });
await mkdir(testRoot, { recursive: true });
protect(testRoot, true);
const diagnostic = resolve(testRoot, 'Talking Quill Stale Schema2 Diagnostic.exe');
const auditPath = resolve(testRoot, 'audit.jsonl');
const diagnosticPath = resolve(testRoot, 'diagnostic.jsonl');
await copyFile(packagedDiagnostic, diagnostic);
await writeFile(auditPath, '');
await writeFile(diagnosticPath, '');
for (const path of [diagnostic, auditPath, diagnosticPath]) protect(path, false);

const environment = {
  ...process.env,
  TQ_STALE_SCHEMA2_AUDIT_PATH: auditPath,
  TQ_STALE_SCHEMA2_DIAGNOSTIC_PATH: diagnosticPath,
};
const invoke = (path, arguments_, env = environment) =>
  spawnSync(path, arguments_, {
    env,
    timeout: 30_000,
    windowsHide: true,
  });
const canonicalResult = invoke(canonical, ['/TQ-DIAGNOSE-STALE-SCHEMA2'], {});
if (canonicalResult.error !== undefined || canonicalResult.status !== 64) {
  throw new Error('canonical package did not reject the diagnostic command with exit 64');
}
const beforeRejectedDispatch = await evidenceLengths();
const rejectedDispatch = invoke(diagnostic, ['/TQ-DIAGNOSE-STALE-SCHEMA2', '/S']);
if (rejectedDispatch.error !== undefined || rejectedDispatch.status !== 64) {
  throw new Error('diagnostic package accepted non-exact argv');
}
if (JSON.stringify(await evidenceLengths()) !== JSON.stringify(beforeRejectedDispatch)) {
  throw new Error('non-exact diagnostic dispatch wrote evidence');
}
const before = await snapshot();
const result = invoke(diagnostic, ['/TQ-DIAGNOSE-STALE-SCHEMA2']);
const expectedRejectionStage = process.env.TQ_STALE_SCHEMA2_EXPECT_REJECTION_STAGE;
const allowedStatuses = expectedRejectionStage === undefined ? [0] : [78];
if (result.error !== undefined || !allowedStatuses.includes(result.status ?? -1)) {
  const detail = result.error?.message || result.stderr?.toString() || String(result.status);
  throw new Error(`packaged diagnostic failed: ${detail}`);
}
const after = await snapshot();
if (JSON.stringify(before.immutable) !== JSON.stringify(after.immutable)) {
  throw new Error('packaged diagnosis changed state outside its audit and diagnostic files');
}
if (before.audit.size === after.audit.size || before.diagnostic.size === after.diagnostic.size) {
  throw new Error('packaged diagnosis did not append both protected evidence files');
}
verifyDiagnosticChain(
  await readFile(diagnosticPath, 'utf8'),
  result.status,
  expectedRejectionStage,
  expectedTopology,
);
verifyAuditChain(await readFile(auditPath, 'utf8'), result.status);
productionNamespaceLock.stdin.end('\n');
console.log('Packaged stale schema-2 diagnostic snapshot and chain checks passed');

async function acquireProductionNamespaceLock() {
  const owner = spawn(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      "$m=[Threading.Mutex]::new($false,'Global\\TalkingQuill.MachineLockTests.V1');try{if(-not $m.WaitOne(300000)){exit 2};[Console]::Out.WriteLine('ready');[Console]::Out.Flush();[Console]::In.ReadLine()|Out-Null}finally{try{$m.ReleaseMutex()}catch{};$m.Dispose()}",
    ],
    { stdio: ['pipe', 'pipe', 'inherit'], windowsHide: true },
  );
  let output = '';
  for await (const chunk of owner.stdout) {
    output += chunk;
    if (output.includes('ready')) return owner;
  }
  throw new Error('production machine-lock test serializer did not start');
}

async function rebuildCurrentArtifacts() {
  const productionTarget = resolve(root, 'tmp', 'cargo-target', 'windows-setup-production');
  const cleanupTarget = resolve(root, 'tmp', 'cargo-target', 'windows-setup-stale-schema2-cleanup');
  const productionOutput = resolve(root, 'tmp', 'windows-setup', architecture);
  const cleanupOutput = resolve(root, 'tmp', 'windows-setup-stale-schema2-cleanup', architecture);
  await Promise.all([
    rm(productionTarget, { recursive: true, force: true }),
    rm(cleanupTarget, { recursive: true, force: true }),
    rm(productionOutput, { recursive: true, force: true }),
    rm(cleanupOutput, { recursive: true, force: true }),
    rm(canonical, { force: true }),
    rm(packagedDiagnostic, { force: true }),
    rm(`${packagedDiagnostic}.nonpromotable.json`, { force: true }),
  ]);
  const sourceEnvironment = {
    ...process.env,
    TALKING_QUILL_RELEASE_COMMIT: sourceCommit,
    TALKING_QUILL_RELEASE_TREE: sourceTree,
  };
  runNode('scripts/build-windows-setup.mjs', [architecture], sourceEnvironment);
  runNode('scripts/build-windows-stale-schema2-cleanup-setup.mjs', [architecture], {
    ...sourceEnvironment,
    TALKING_QUILL_STALE_SCHEMA2_CLEANUP_BUILD: '1',
  });
  runNode('scripts/pack-windows-native.mjs', [architecture, 'release'], {
    ...sourceEnvironment,
    TALKING_QUILL_PACKAGE_MODE: 'fresh',
  });
  runNode('scripts/pack-windows-native.mjs', [architecture, 'release'], {
    ...sourceEnvironment,
    TALKING_QUILL_PACKAGE_MODE: 'stale-schema2-cleanup',
    TALKING_QUILL_STALE_SCHEMA2_CLEANUP_BUILD: '1',
  });
}

function runNode(script, arguments_, environment) {
  const result = spawnSync(process.execPath, [resolve(root, script), ...arguments_], {
    cwd: root,
    env: environment,
    stdio: 'inherit',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`${script} failed before packaged runtime testing`);
}

function git(arguments_) {
  const result = spawnSync('git', arguments_, {
    cwd: root,
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(result.stderr || `git ${arguments_.join(' ')} failed`);
  return result.stdout.trim();
}

function protect(path, directory) {
  const sddl = directory
    ? 'O:BAG:BAD:P(A;OICI;FA;;;SY)(A;OICI;FA;;;BA)'
    : 'O:BAG:BAD:P(A;;FA;;;SY)(A;;FA;;;BA)';
  const command = `
    Add-Type -TypeDefinition @'
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
public static class TqProtectedAcl {
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool ConvertStringSecurityDescriptorToSecurityDescriptor(
    string text, uint revision, out IntPtr descriptor, IntPtr size);
  [DllImport("advapi32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  static extern bool SetFileSecurity(string path, uint information, IntPtr descriptor);
  [DllImport("kernel32.dll")]
  static extern IntPtr LocalFree(IntPtr memory);
  public static void Apply(string path, string sddl) {
    IntPtr descriptor;
    if (!ConvertStringSecurityDescriptorToSecurityDescriptor(sddl, 1, out descriptor, IntPtr.Zero))
      throw new Win32Exception(Marshal.GetLastWin32Error());
    try {
      if (!SetFileSecurity(path, 0x80000005, descriptor))
        throw new Win32Exception(Marshal.GetLastWin32Error());
    } finally { LocalFree(descriptor); }
  }
}
'@
    [TqProtectedAcl]::Apply($env:TQ_E2E_PATH, $env:TQ_E2E_SDDL)
  `;
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', command],
    {
      env: { ...process.env, TQ_E2E_PATH: path, TQ_E2E_SDDL: sddl },
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (result.status !== 0) throw new Error(`cannot protect ${path}: ${result.stderr}`);
}

async function evidenceLengths() {
  const [audit, diagnosticLog] = await Promise.all([readFile(auditPath), readFile(diagnosticPath)]);
  return { audit: audit.length, diagnostic: diagnosticLog.length };
}

async function snapshot() {
  const [packageBytes, audit, diagnosticLog] = await Promise.all([
    readFile(diagnostic),
    readFile(auditPath),
    readFile(diagnosticPath),
  ]);
  const machine = powershellJson(`
    $items = @()
    foreach ($base in @($env:ProgramFiles, $env:ProgramData)) {
      $roots = Get-ChildItem -LiteralPath $base -Force -ErrorAction Stop |
        Where-Object { $_.Name -like '*Talking Quill*' }
      foreach ($root in $roots) {
        $entries = @($root)
        if ($root.PSIsContainer) {
          $entries += Get-ChildItem -LiteralPath $root.FullName -Force -Recurse -ErrorAction Stop
        }
        foreach ($entry in $entries) {
          $digest = if ($entry.PSIsContainer) { $null } else {
            (Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash
          }
          $items += [pscustomobject]@{
            Path=$entry.FullName
            Attributes=[string]$entry.Attributes
            Length=$entry.Length
            Sddl=(Get-Acl -LiteralPath $entry.FullName).Sddl
            Sha256=$digest
          }
        }
      }
    }
    $registry = & reg.exe query 'HKLM\\Software\\Talking Quill' /s 2>&1 | Out-String
    $run = & reg.exe query 'HKLM\\Software\\Microsoft\\Windows\\CurrentVersion\\Run' 2>&1 | Out-String
    $services = Get-Service | Where-Object { $_.Name -like '*TalkingQuill*' } |
      Select-Object Name, Status, StartType
    $tasks = & schtasks.exe /Query /FO CSV /V 2>&1 | Select-String 'TalkingQuill' | Out-String
    [pscustomobject]@{
      Items=$items
      Registry=$registry
      Run=$run
      Services=$services
      Tasks=$tasks
    } | ConvertTo-Json -Compress -Depth 8
  `);
  const packageAcl = powershellText('(Get-Acl -LiteralPath $env:TQ_E2E_PATH).Sddl', {
    TQ_E2E_PATH: diagnostic,
  });
  return {
    immutable: {
      machine,
      packageAcl,
      packageSha256: hash(packageBytes),
      packageSize: packageBytes.length,
    },
    audit: { size: audit.length },
    diagnostic: { size: diagnosticLog.length },
  };
}

function powershellJson(command) {
  return JSON.parse(powershellText(command));
}

function powershellText(command, extraEnvironment = {}) {
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', command],
    {
      env: { ...process.env, ...extraEnvironment },
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (result.status !== 0) throw new Error(result.stderr || 'PowerShell snapshot failed');
  return result.stdout.trim();
}

function verifyDiagnosticChain(text, status, expectedRejectionStage, expectedTopology) {
  const lines = text.trim().split('\n').map(JSON.parse);
  if (lines.length === 0) throw new Error('diagnostic chain is empty');
  const first = lines[0];
  let chain = createHash('sha256')
    .update('TalkingQuill/stale-schema2-diagnostic-chain/v1\0')
    .update(first.diagnosticIdentity)
    .update(Buffer.alloc(0))
    .update(first.operationId)
    .digest();
  let sequence = 0;
  for (const event of lines) {
    sequence += 1;
    if (event.sequence !== sequence || event.previousSha256 !== chain.toString('hex')) {
      throw new Error('diagnostic sequence or previous hash is invalid');
    }
    const unsigned = { ...event };
    delete unsigned.eventSha256;
    chain = createHash('sha256').update(chain).update(JSON.stringify(unsigned)).digest();
    if (event.eventSha256 !== chain.toString('hex')) {
      throw new Error('diagnostic event hash is invalid');
    }
  }
  const last = lines.at(-1);
  if (
    (status === 0 &&
      (last?.stageCode !== 'diagnostic.complete' || last?.evidence?.state !== expectedTopology)) ||
    (status === 78 && (last?.outcome !== 'rejected' || last?.stageCode !== expectedRejectionStage))
  ) {
    throw new Error('diagnostic terminal event does not match its exit status');
  }
  if (status === 0) {
    const registry = lines.find((event) => event.stageCode === 'registry.inventory');
    const evidence = registry?.evidence;
    if (
      registry?.outcome !== 'passed' ||
      evidence?.aclAdmission !== 'legacy-exact-parent' ||
      !/^[0-9a-f]{32}$/u.test(evidence?.machineLockSuffix ?? '') ||
      JSON.stringify(evidence?.subkeys) !== JSON.stringify(['RecoveryStateLockV1']) ||
      JSON.stringify(evidence?.values) !== JSON.stringify([]) ||
      evidence?.parentDescriptor !== evidence?.childDescriptor
    ) {
      throw new Error('diagnostic did not admit the exact retained legacy registry fixture');
    }
  }
}

function verifyAuditChain(text, status) {
  const lines = text.trim().split('\n').map(JSON.parse);
  if (
    lines[0]?.stage !== 'diagnostic-start' ||
    (status === 0 && lines.at(-1)?.stage !== 'diagnostic-complete') ||
    lines.length !== (status === 0 ? 2 : 1)
  ) {
    throw new Error('diagnostic audit inventory is not exact');
  }
  const first = lines[0];
  let chain = createHash('sha256')
    .update('TalkingQuill/stale-schema2-audit-chain/v1\0')
    .update(first.auditIdentity)
    .update(Buffer.alloc(0))
    .update(first.operationId)
    .digest();
  for (const event of lines) {
    if (event.previousSha256 !== chain.toString('hex')) {
      throw new Error('audit previous hash is invalid');
    }
    chain = createHash('sha256')
      .update(chain)
      .update(event.stage)
      .update(event.bindingSha256)
      .update(event.proofSha256)
      .digest();
    if (event.eventSha256 !== chain.toString('hex')) {
      throw new Error('audit event hash is invalid');
    }
  }
}

function hash(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}
