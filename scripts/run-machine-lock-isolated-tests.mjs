import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { existsSync, readdirSync, rmdirSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';

const KNOWN_LEAKED_TEST_NAMESPACE_IDS = ['21703e4406cd272de859f7e313080675'];
const separator = process.argv.indexOf('--');
if (separator === -1 || separator === process.argv.length - 1) {
  throw new Error('Usage: run-machine-lock-isolated-tests.mjs -- <command>');
}
const command = process.argv.slice(separator + 1).join(' ');

if (process.platform !== 'win32') {
  const result = spawnSync(command, { shell: true, stdio: 'inherit' });
  process.exit(result.status ?? 1);
}

const serializer = await acquireSerializer();
let status = 1;
let productionBefore;
const testNamespaceId = randomBytes(16).toString('hex');
process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID = testNamespaceId;
try {
  for (const leakedId of KNOWN_LEAKED_TEST_NAMESPACE_IDS) {
    removeKnownLeakedTestNamespace(leakedId);
  }
  removeEmptyTestRoots();
  removeEmptyTestRegistryRoot();
  assertNoTestNamespaceLeftovers('before');
  assertTestNamespaceAbsent(testNamespaceId, 'before');
  productionBefore = productionResidueSnapshot();
  const result = spawnSync(command, { shell: true, stdio: 'inherit', windowsHide: true });
  status = result.status ?? 1;
  if (result.error) throw result.error;
  if (result.signal) throw new Error(`isolated test command ended with ${result.signal}`);
} finally {
  let productionFailure;
  try {
    if (productionBefore !== undefined && productionResidueSnapshot() !== productionBefore) {
      productionFailure = new Error('isolated tests changed production machine-lock residue');
    }
  } catch (error) {
    productionFailure = error;
  }
  try {
    removeTestNamespace(testNamespaceId);
    assertTestNamespaceAbsent(testNamespaceId, 'after');
    removeEmptyTestRoots();
    removeEmptyTestRegistryRoot();
    assertNoTestNamespaceLeftovers('after');
  } finally {
    serializer.stdin.end('\n');
  }
  if (productionFailure) throw productionFailure;
}
process.exit(status);

async function acquireSerializer() {
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
  throw new Error('machine-lock test serializer did not start');
}

function removeKnownLeakedTestNamespace(id) {
  assertNamespaceId(id);
  const processProof = powershellJson(
    String.raw`
    $id=$env:TQ_INSPECT_NAMESPACE_ID
    $wrapper=[uint32]$env:TQ_WRAPPER_PROCESS_ID
    $serializer=[uint32]$env:TQ_SERIALIZER_PROCESS_ID
    $parent=[uint32]$env:TQ_WRAPPER_PARENT_PROCESS_ID
    $matches=@(Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object {
      $command=[string]$_.CommandLine
      $_.ProcessId -ne $PID -and $_.ProcessId -ne $wrapper -and
      $_.ProcessId -ne $serializer -and $_.ProcessId -ne $parent -and (
        ($_.Name -match '(?i)^talking[_-]quill.*\.exe$') -or
        ($command -match '(?i)cargo(?:\.exe)?\s+test.+(?:helper|windows-setup)')
      )
    } | Select-Object ProcessId,ExecutablePath,CommandLine)
    $hklm=Test-Path -LiteralPath "Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill Tests\$id"
    [pscustomobject]@{Processes=$matches;HklmTestNamespace=$hklm}|ConvertTo-Json -Compress -Depth 4
  `,
    {
      TQ_INSPECT_NAMESPACE_ID: id,
      TQ_WRAPPER_PROCESS_ID: String(process.pid),
      TQ_SERIALIZER_PROCESS_ID: String(serializer.pid),
      TQ_WRAPPER_PARENT_PROCESS_ID: String(process.ppid),
    },
  );
  if (processProof.HklmTestNamespace || processProof.Processes.length !== 0) {
    throw new Error(
      `known leaked test namespace ${id} is active or has HKLM state: ${JSON.stringify(processProof)}`,
    );
  }
  removeTestNamespace(id);
  assertTestNamespaceAbsent(id, 'known-leak-cleanup');
}

function removeTestNamespace(id) {
  assertNamespaceId(id);
  for (const path of namespacePaths(id)) {
    if (!existsSync(path)) continue;
    const owner = spawnSync('takeown.exe', ['/f', path, '/r', '/d', 'y'], {
      encoding: 'utf8',
      windowsHide: true,
    });
    const account = `${process.env.USERDOMAIN}\\${process.env.USERNAME}`;
    const access = spawnSync('icacls.exe', [path, '/grant', `${account}:(OI)(CI)F`, '/t', '/c'], {
      encoding: 'utf8',
      windowsHide: true,
    });
    if (owner.status !== 0 || access.status !== 0) {
      throw new Error(`cannot take ownership of machine-lock test namespace ${path}`);
    }
    rmSync(path, { recursive: true, force: true });
  }
  const result = spawnSync(
    'reg.exe',
    ['delete', `HKCU\\Software\\Talking Quill Tests\\${id}`, '/f'],
    { encoding: 'utf8', windowsHide: true },
  );
  if (![0, 1].includes(result.status ?? -1)) {
    throw new Error(`cannot remove machine-lock test registry namespace ${id}`);
  }
}

function assertTestNamespaceAbsent(id, phase) {
  assertNamespaceId(id);
  const state = powershellJson(
    String.raw`
    $id=$env:TQ_INSPECT_NAMESPACE_ID
    $hkcu=Test-Path -LiteralPath "Registry::HKEY_CURRENT_USER\Software\Talking Quill Tests\$id"
    $hklm=Test-Path -LiteralPath "Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill Tests\$id"
    [pscustomobject]@{Hkcu=$hkcu;Hklm=$hklm}|ConvertTo-Json -Compress
  `,
    { TQ_INSPECT_NAMESPACE_ID: id },
  );
  const paths = namespacePaths(id).filter(existsSync);
  if (state.Hkcu || state.Hklm || paths.length !== 0) {
    throw new Error(
      `machine-lock test namespace ${id} remains ${phase}: ${JSON.stringify({ state, paths })}`,
    );
  }
}

function removeEmptyTestRegistryRoot() {
  const state = powershellJson(String.raw`
    $path='HKCU\Software\Talking Quill Tests'
    $provider='Registry::HKEY_CURRENT_USER\Software\Talking Quill Tests'
    if(-not (Test-Path -LiteralPath $provider)){
      [pscustomobject]@{Removed=$false;Absent=$true}|ConvertTo-Json -Compress
      exit 0
    }
    $key=Get-Item -LiteralPath $provider -ErrorAction Stop
    $children=@(Get-ChildItem -LiteralPath $provider -ErrorAction Stop)
    $values=@($key.GetValueNames())
    if($children.Count-ne0 -or $values.Count-ne0){
      throw 'machine-lock test registry root is not empty'
    }
    & reg.exe delete $path /f | Out-Null
    if($LASTEXITCODE-ne0){throw 'cannot remove empty machine-lock test registry root'}
    [pscustomobject]@{Removed=$true;Absent=(-not(Test-Path -LiteralPath $provider))}|
      ConvertTo-Json -Compress
  `);
  if (!state.Absent) throw new Error('empty machine-lock test registry root remains');
}

function removeEmptyTestRoots() {
  for (const path of [
    resolve('tmp', 'machine-lock-tests', 'orphan-inventory'),
    resolve('tmp', 'machine-lock-tests', 'windows-setup-unit'),
    resolve('tmp', 'machine-lock-tests', 'windows-setup'),
    resolve('tmp', 'machine-lock-tests', 'helper'),
    resolve('tmp', 'machine-lock-tests'),
  ]) {
    if (existsSync(path) && readdirSync(path).length === 0) rmdirSync(path);
  }
}

function assertNoTestNamespaceLeftovers(phase) {
  const state = powershellJson(String.raw`
    $registryRoot='Registry::HKEY_CURRENT_USER\Software\Talking Quill Tests'
    $rootPresent=Test-Path -LiteralPath $registryRoot
    $registry=@(Get-ChildItem -LiteralPath $registryRoot -ErrorAction SilentlyContinue |
      Where-Object {$_.PSChildName -match '^[0-9a-f]{32}$'} | ForEach-Object PSChildName)
    [pscustomobject]@{RootPresent=$rootPresent;Registry=$registry}|ConvertTo-Json -Compress
  `);
  const roots = [
    resolve('tmp', 'machine-lock-tests', 'helper'),
    resolve('tmp', 'machine-lock-tests', 'windows-setup'),
    resolve('tmp', 'machine-lock-tests', 'orphan-inventory'),
    resolve('tmp', 'machine-lock-tests', 'windows-setup-unit'),
  ];
  const paths = powershellJson(
    String.raw`
    $roots=@($env:TQ_TEST_ROOTS -split '\|')
    $items=@(foreach($root in $roots){
      Get-ChildItem -LiteralPath $root -Force -ErrorAction SilentlyContinue |
        Where-Object {$_.Name -match '^[0-9a-f]{32}$'} | ForEach-Object FullName
    })
    [pscustomobject]@{Paths=$items}|ConvertTo-Json -Compress
  `,
    { TQ_TEST_ROOTS: roots.join('|') },
  ).Paths;
  if (state.RootPresent || state.Registry.length !== 0 || paths.length !== 0) {
    throw new Error(
      `machine-lock test namespace leftovers ${phase}: ${JSON.stringify({ registry: state.Registry, paths })}`,
    );
  }
}

function productionResidueSnapshot() {
  return powershellText(String.raw`
    $pd=[Environment]::GetFolderPath('CommonApplicationData')
    $roots=@(Get-ChildItem -LiteralPath $pd -Force -ErrorAction Stop | Where-Object {
      $_.Name -like '.Talking Quill.machine-lock-*' -or
      $_.Name -like '.Talking Quill.machine-lock-pending-*' -or
      $_.Name -like '.Talking Quill.machine-lifecycle-retained-*'
    })
    $items=@()
    foreach($root in $roots) {
      $entries=@($root)
      if($root.PSIsContainer) {
        $entries+=@(Get-ChildItem -LiteralPath $root.FullName -Force -Recurse -ErrorAction Stop)
      }
      foreach($entry in $entries) {
        $fileId=(& fsutil.exe file queryfileid $entry.FullName 2>&1 | Out-String).Trim()
        $sha256=if($entry.PSIsContainer){$null}else{(Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash}
        $items+=[pscustomobject]@{
          Path=$entry.FullName
          Attributes=[string]$entry.Attributes
          Length=$entry.Length
          Sddl=(Get-Acl -LiteralPath $entry.FullName).Sddl
          FileId=$fileId
          Sha256=$sha256
        }
      }
    }
    $items=@($items|Sort-Object Path)
    $registry=& reg.exe query 'HKLM\Software\Talking Quill\RecoveryStateLockV1' /s 2>&1 | Out-String
    $parentAcl=if(Test-Path 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill'){
      (Get-Acl 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill').Sddl
    }else{$null}
    $childAcl=if(Test-Path 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill\RecoveryStateLockV1'){
      (Get-Acl 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill\RecoveryStateLockV1').Sddl
    }else{$null}
    [pscustomobject]@{Items=$items;Registry=$registry;ParentAcl=$parentAcl;ChildAcl=$childAcl}|
      ConvertTo-Json -Compress -Depth 6
  `);
}

function namespacePaths(id) {
  return [
    resolve('tmp', 'machine-lock-tests', 'helper', id),
    resolve('tmp', 'machine-lock-tests', 'windows-setup', id),
    resolve('tmp', 'machine-lock-tests', 'orphan-inventory', id),
    resolve('tmp', 'machine-lock-tests', 'windows-setup-unit', id),
  ];
}

function assertNamespaceId(id) {
  if (!/^[0-9a-f]{32}$/u.test(id)) throw new Error('invalid machine-lock test namespace');
}

function powershellJson(command, environment = {}) {
  return JSON.parse(powershellText(command, environment));
}

function powershellText(command, environment = {}) {
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', command],
    {
      env: { ...process.env, ...environment },
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (result.status !== 0)
    throw new Error(result.stderr || 'PowerShell namespace inspection failed');
  return result.stdout.trim();
}
