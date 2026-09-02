import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import {
  closeSync,
  existsSync,
  fsyncSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmdirSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, relative, resolve } from 'node:path';

const RECORD_SCHEMA = 1;
const root = resolve(import.meta.dirname, '..');
const stateRoot = resolve(root, 'tmp', 'machine-lock-tests');
const recordRoot = resolve(stateRoot, '.cleanup-records-v1');
const rootKinds = ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit'];
const separator = process.argv.indexOf('--');
if (separator === -1 || separator === process.argv.length - 1) {
  throw new Error('Usage: run-machine-lock-isolated-tests.mjs -- <command>');
}
const command = process.argv.slice(separator + 1).join(' ');

if (process.platform !== 'win32') {
  const result = spawnSync(command, { shell: true, stdio: 'inherit' });
  process.exit(result.status ?? 1);
}

const currentUserSid = powershellText(
  '[Security.Principal.WindowsIdentity]::GetCurrent().User.Value',
);
const deleter = buildTreeDeleter();
const serializer = await acquireSerializer();
let status = 1;
let productionBefore;
let record;
try {
  recoverRecordedNamespaces(deleter);
  assertNoUnknownNamespaces();
  productionBefore = productionResidueSnapshot();
  record = prepareNamespace(randomBytes(16).toString('hex'));
  process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID = record.namespaceId;
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'roots-created') process.exit(197);
  const child = spawn(deleter, ['--supervise', command], {
    shell: false,
    stdio: 'inherit',
    windowsHide: true,
    env: process.env,
  });
  record.child = processIdentity(child.pid);
  record.phase = 'child-running';
  writeRecord(record);
  const result = await new Promise((done) => {
    child.once('error', (error) => done({ error }));
    child.once('exit', (code, signal) => done({ code, signal }));
  });
  record.phase = 'child-dead';
  writeRecord(record);
  if (result.error) throw result.error;
  if (result.signal) throw new Error(`isolated test command ended with ${result.signal}`);
  status = result.code ?? 1;
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
    if (record !== undefined) cleanupRecord(record, deleter, true);
    recoverRecordedNamespaces(deleter);
    assertNoUnknownNamespaces();
  } finally {
    serializer.stdin.end('\n');
  }
  if (productionFailure) throw productionFailure;
}
process.exit(status);

function buildTreeDeleter() {
  const result = spawnSync(
    'cargo',
    [
      'build',
      '--manifest-path',
      resolve(root, 'helper', 'Cargo.toml'),
      '--locked',
      '--target-dir',
      resolve(root, 'tmp', 'cargo-target', 'machine-lock-test-wrapper'),
      '-p',
      'talking-quill-helper',
      '--features',
      'machine-lock-test-namespace',
      '--bin',
      'talking-quill-test-tree-delete',
    ],
    { cwd: root, stdio: 'inherit', windowsHide: true },
  );
  if (result.status !== 0) throw new Error('cannot build machine-lock test tree deleter');
  const path = resolve(
    root,
    'tmp',
    'cargo-target',
    'machine-lock-test-wrapper',
    'debug',
    'talking-quill-test-tree-delete.exe',
  );
  if (!isPlainPath(path, false)) throw new Error('machine-lock test tree deleter is unavailable');
  return path;
}

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

function prepareNamespace(namespaceId) {
  assertNamespaceId(namespaceId);
  ensureProtectedRecordRoot();
  const record = {
    schemaVersion: RECORD_SCHEMA,
    recordId: randomBytes(16).toString('hex'),
    namespaceId,
    phase: 'prepared',
    owner: processIdentity(process.pid),
    child: null,
    projectRootIdentity: fileIdentity(root),
    stateRootIdentity: fileIdentity(stateRoot),
    recordDirectoryIdentity: fileIdentity(recordRoot),
    recordDirectoryAcl: pathAcl(recordRoot),
    registryPath: `HKCU\\Software\\Talking Quill Tests\\${namespaceId}\\RecoveryStateLockV1`,
    creatingRoot: null,
    deletingRoot: null,
    deletedRoots: [],
    roots: [],
  };
  writeRecord(record);
  for (const kind of rootKinds) {
    const path = namespaceRoot(kind, namespaceId);
    mkdirSync(dirname(path), { recursive: true });
    if (existsSync(path)) throw new Error(`machine-lock test root already exists: ${path}`);
    const rootRecord = { kind, identity: null, inventory: [] };
    record.roots.push(rootRecord);
    record.creatingRoot = kind;
    writeRecord(record);
    mkdirSync(path);
    if (!isPlainPath(path, true)) throw new Error(`machine-lock test root is not plain: ${path}`);
    rootRecord.identity = ownedTreeIdentity(path);
    record.creatingRoot = null;
    writeRecord(record);
  }
  createTestRegistryNamespace(namespaceId);
  record.phase = 'roots-created';
  writeRecord(record);
  return record;
}

function createTestRegistryNamespace(namespaceId) {
  powershellText(
    String.raw`
    $id=$env:TQ_NAMESPACE_ID
    $key=[Microsoft.Win32.Registry]::CurrentUser.CreateSubKey("Software\Talking Quill Tests\$id",$true)
    try{
      $security=New-Object Security.AccessControl.RegistrySecurity
      $security.SetAccessRuleProtection($true,$false)
      foreach($sid in @($env:TQ_CURRENT_USER_SID,'S-1-5-18','S-1-5-32-544','S-1-5-11')){
        $identity=New-Object Security.Principal.SecurityIdentifier($sid)
        $rule=New-Object Security.AccessControl.RegistryAccessRule($identity,'FullControl','None','None','Allow')
        $security.AddAccessRule($rule)
      }
      $key.SetAccessControl($security)
      $key.Flush()
    }finally{$key.Dispose()}
  `,
    { TQ_NAMESPACE_ID: namespaceId, TQ_CURRENT_USER_SID: currentUserSid },
  );
}

function ensureProtectedRecordRoot() {
  mkdirSync(recordRoot, { recursive: true });
  for (const path of [root, resolve(root, 'tmp'), stateRoot, recordRoot]) {
    if (!isPlainPath(path, true)) {
      throw new Error(`machine-lock cleanup record ancestor is not plain: ${path}`);
    }
  }
  applyExactRecordAcl(recordRoot, true);
  fsyncDirectory(recordRoot);
}

function applyExactRecordAcl(path, directory) {
  const inheritance = directory ? '(OI)(CI)F' : 'F';
  const result = spawnSync(
    'icacls.exe',
    [
      path,
      '/inheritance:r',
      '/grant:r',
      `*${currentUserSid}:${inheritance}`,
      `*S-1-5-18:${inheritance}`,
      `*S-1-5-32-544:${inheritance}`,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`cannot protect machine-lock cleanup record: ${path}`);
}

function assertProtectedRecord(path) {
  const state = powershellJson(
    String.raw`
    $acl=Get-Acl -LiteralPath $env:TQ_RECORD_PATH -ErrorAction Stop
    $allowed=@($env:TQ_CURRENT_USER_SID,'S-1-5-18','S-1-5-32-544')
    $bad=@($acl.Access|Where-Object {
      $sid=$_.IdentityReference.Translate([Security.Principal.SecurityIdentifier]).Value
      $sid-notin$allowed -or [string]$_.AccessControlType-ne'Allow' -or [string]$_.FileSystemRights-ne'FullControl'
    })
    [pscustomobject]@{Owner=$acl.Owner;Protected=$acl.AreAccessRulesProtected;Bad=$bad.Count;Count=@($acl.Access).Count}|ConvertTo-Json -Compress
  `,
    { TQ_RECORD_PATH: path, TQ_CURRENT_USER_SID: currentUserSid },
  );
  const ownerSid = powershellText(
    '(New-Object Security.Principal.NTAccount($env:TQ_RECORD_OWNER)).Translate([Security.Principal.SecurityIdentifier]).Value',
    { TQ_RECORD_OWNER: state.Owner },
  );
  if (!state.Protected || state.Bad !== 0 || state.Count !== 3 || ownerSid !== currentUserSid) {
    throw new Error(`cleanup record ACL is not exact: ${path}`);
  }
}

function writeRecord(record) {
  validateRecord(record);
  ensureProtectedRecordRoot();
  const path = recordPath(record.recordId);
  const temporary = `${path}.tmp-${randomBytes(8).toString('hex')}`;
  const bytes = `${JSON.stringify(record)}\n`;
  const handle = openSync(temporary, 'wx');
  try {
    writeFileSync(handle, bytes, 'utf8');
    fsyncSync(handle);
  } finally {
    closeSync(handle);
  }
  applyExactRecordAcl(temporary, false);
  renameSync(temporary, path);
  fsyncDirectory(recordRoot);
}

function recoverRecordedNamespaces(deleter) {
  if (!existsSync(recordRoot)) return;
  if (!isPlainPath(recordRoot, true)) throw new Error('cleanup record root is not plain');
  const files = readdirSync(recordRoot).sort();
  for (const name of files) {
    if (!/^[0-9a-f]{32}\.json$/u.test(name)) {
      throw new Error(`unknown machine-lock cleanup record: ${name}`);
    }
  }
  const records = files.map((name) => readRecord(resolve(recordRoot, name)));
  const ids = new Set();
  for (const value of records) {
    if (ids.has(value.namespaceId))
      throw new Error('duplicate machine-lock cleanup namespace record');
    ids.add(value.namespaceId);
  }
  assertNoUnknownNamespaces(ids);
  for (const value of records) {
    if (processIdentityAlive(value.owner) || processIdentityAlive(value.child)) {
      throw new Error(`machine-lock cleanup record is owned by a live process: ${value.recordId}`);
    }
    cleanupRecord(value, deleter, false);
  }
  removeEmptyParents();
}

function cleanupRecord(record, deleter, ownerMayBeCurrent) {
  validateRecord(record);
  if (!ownerMayBeCurrent && processIdentityAlive(record.owner)) {
    throw new Error('cannot recover a live machine-lock test wrapper');
  }
  if (processIdentityAlive(record.child)) {
    throw new Error('cannot clean a live machine-lock test child');
  }
  assertNoRelevantTestProcesses();
  if (
    fileIdentity(root) !== record.projectRootIdentity ||
    fileIdentity(stateRoot) !== record.stateRootIdentity ||
    fileIdentity(recordRoot) !== record.recordDirectoryIdentity ||
    pathAcl(recordRoot) !== record.recordDirectoryAcl
  ) {
    throw new Error('machine-lock cleanup record ancestor identity changed');
  }
  const sealingInventory = ![
    'inventory-sealed',
    'deleting-root',
    'filesystem-deleted',
    'deleting-registry',
    'registry-deleted',
  ].includes(record.phase);
  const registryInventory = sealingInventory
    ? registrySnapshot(record.namespaceId)
    : record.registryInventory;
  for (const rootRecord of record.roots) {
    const path = namespaceRoot(rootRecord.kind, record.namespaceId);
    if (!existsSync(path)) {
      if (rootRecord.identity === null && record.creatingRoot === rootRecord.kind) {
        record.deletedRoots.push(rootRecord.kind);
        record.creatingRoot = null;
        continue;
      }
      if (
        record.deletedRoots.includes(rootRecord.kind) ||
        record.deletingRoot === rootRecord.kind
      ) {
        if (!record.deletedRoots.includes(rootRecord.kind))
          record.deletedRoots.push(rootRecord.kind);
        record.deletingRoot = null;
        continue;
      }
      throw new Error(`recorded machine-lock root is missing: ${path}`);
    }
    if (rootRecord.identity === null && record.creatingRoot === rootRecord.kind) {
      rootRecord.identity = ownedTreeIdentity(path);
      record.creatingRoot = null;
    }
    if (ownedTreeIdentity(path) !== rootRecord.identity) {
      throw new Error(`recorded machine-lock root identity changed: ${path}`);
    }
    const inventory = inspectTree(path);
    if (
      !sealingInventory &&
      JSON.stringify(inventory) !== JSON.stringify(rootRecord.inventory) &&
      !(
        record.deletingRoot === rootRecord.kind &&
        exactInventorySubset(inventory, rootRecord.inventory)
      )
    ) {
      throw new Error(`machine-lock test tree changed after inventory seal: ${path}`);
    }
    if (sealingInventory) rootRecord.inventory = inventory;
  }
  record.registryInventory = registryInventory;
  record.phase = 'inventory-sealed';
  writeRecord(record);
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'inventory-sealed') process.exit(197);

  for (const rootRecord of record.roots) {
    if (record.deletedRoots.includes(rootRecord.kind)) continue;
    const path = namespaceRoot(rootRecord.kind, record.namespaceId);
    const resuming = record.deletingRoot === rootRecord.kind;
    record.phase = 'deleting-root';
    record.deletingRoot = rootRecord.kind;
    writeRecord(record);
    const result = spawnSync(
      deleter,
      [resuming ? '--resume-exact' : '--exact', path, rootRecord.identity],
      {
        input: JSON.stringify(rootRecord.inventory),
        encoding: 'utf8',
        windowsHide: true,
      },
    );
    if (result.status !== 0) {
      throw new Error(`handle-bound machine-lock test tree deletion failed: ${path}`);
    }
    record.deletedRoots.push(rootRecord.kind);
    record.deletingRoot = null;
    writeRecord(record);
    if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === `deleted-root:${rootRecord.kind}`) {
      process.exit(197);
    }
  }
  record.phase = 'filesystem-deleted';
  writeRecord(record);
  record.phase = 'deleting-registry';
  writeRecord(record);
  const registryNow = registrySnapshot(record.namespaceId);
  if (registryNow !== '{"Present":false}') {
    deleteExactRegistry(record.namespaceId, registryInventory);
  }
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'registry-deleted') process.exit(197);
  record.phase = 'registry-deleted';
  writeRecord(record);
  const path = recordPath(record.recordId);
  if (!isPlainPath(path, false)) throw new Error('cleanup record changed before deletion');
  unlinkSync(path);
  fsyncDirectory(recordRoot);
  removeEmptyParents();
}

function exactInventorySubset(actual, expected) {
  const expectedByPath = new Map(expected.map((entry) => [entry.relativePath, entry]));
  return actual.every((entry) => {
    const recorded = expectedByPath.get(entry.relativePath);
    return recorded !== undefined && JSON.stringify(recorded) === JSON.stringify(entry);
  });
}

function inspectTree(path) {
  if (!isPlainPath(path, true)) throw new Error(`machine-lock test root is not plain: ${path}`);
  const inventory = [];
  inspectChildren(path, path, inventory);
  inventory.sort((left, right) => left.relativePath.localeCompare(right.relativePath));
  return inventory;
}

function inspectChildren(rootPath, parent, inventory) {
  for (const name of readdirSync(parent).sort()) {
    const path = resolve(parent, name);
    const metadata = lstatSync(path, { bigint: true });
    if (metadata.isSymbolicLink() || metadata.isSocket() || metadata.isFIFO()) {
      throw new Error(`reparse or special entry blocks machine-lock cleanup: ${path}`);
    }
    const directory = metadata.isDirectory();
    if (!directory && !metadata.isFile()) {
      throw new Error(`unexpected machine-lock test entry type: ${path}`);
    }
    if (!directory && metadata.nlink !== 1n) {
      throw new Error(`hard-linked machine-lock test entry blocks cleanup: ${path}`);
    }
    const relativePath = relative(rootPath, path).replaceAll('\\', '/');
    if (relativePath === '' || relativePath.startsWith('../') || relativePath.includes('/../')) {
      throw new Error('machine-lock test inventory escaped its root');
    }
    inventory.push({ relativePath, directory, identity: ownedTreeIdentity(path) });
    if (directory) inspectChildren(rootPath, path, inventory);
  }
}

function readRecord(path) {
  if (!isPlainPath(path, false)) throw new Error(`cleanup record is not a plain file: ${path}`);
  assertProtectedRecord(path);
  const record = JSON.parse(readFileSync(path, 'utf8'));
  validateRecord(record);
  if (path !== recordPath(record.recordId)) throw new Error('cleanup record filename is invalid');
  return record;
}

function validateRecord(record) {
  if (record?.schemaVersion !== RECORD_SCHEMA) throw new Error('cleanup record schema is invalid');
  assertNamespaceId(record.namespaceId);
  assertNamespaceId(record.recordId);
  for (const identity of [
    record.projectRootIdentity,
    record.stateRootIdentity,
    record.recordDirectoryIdentity,
  ]) {
    if (!/^[0-9a-f]{32}$/u.test(identity ?? ''))
      throw new Error('cleanup record identity is invalid');
  }
  if (
    record.registryPath !==
    `HKCU\\Software\\Talking Quill Tests\\${record.namespaceId}\\RecoveryStateLockV1`
  ) {
    throw new Error('cleanup record registry path is invalid');
  }
  const kinds = record.roots.map((entry) => entry.kind);
  if (new Set(kinds).size !== kinds.length || kinds.some((kind) => !rootKinds.includes(kind))) {
    throw new Error('cleanup record root inventory is invalid');
  }
  if (record.phase !== 'prepared' && kinds.length !== rootKinds.length) {
    throw new Error('cleanup record root inventory is incomplete');
  }
}

function assertNoUnknownNamespaces(recorded = new Set()) {
  if (existsSync(stateRoot)) {
    if (!isPlainPath(stateRoot, true)) throw new Error('machine-lock test state root is not plain');
    const allowed = new Set([...rootKinds, '.cleanup-records-v1']);
    for (const name of readdirSync(stateRoot)) {
      const path = resolve(stateRoot, name);
      if (!allowed.has(name) || !isPlainPath(path, true)) {
        throw new Error(`unknown machine-lock test state entry fails closed: ${name}`);
      }
    }
  }
  for (const kind of rootKinds) {
    const parent = resolve(stateRoot, kind);
    if (!existsSync(parent)) continue;
    if (!isPlainPath(parent, true))
      throw new Error(`machine-lock test parent is not plain: ${parent}`);
    for (const name of readdirSync(parent)) {
      if (!/^[0-9a-f]{32}$/u.test(name) || !recorded.has(name)) {
        throw new Error(`unknown machine-lock test namespace fails closed: ${kind}/${name}`);
      }
    }
  }
  const registry = registryNamespaceInventory();
  if (registry.Values.length !== 0)
    throw new Error('unknown HKCU machine-lock test root values fail closed');
  for (const id of registry.Ids) {
    if (!recorded.has(id))
      throw new Error(`unknown HKCU machine-lock test namespace fails closed: ${id}`);
  }
}

function registrySnapshot(namespaceId) {
  return powershellText(
    String.raw`
    $id=$env:TQ_NAMESPACE_ID
    function Values($key){@($key.GetValueNames()|Sort-Object|ForEach-Object {
      $kind=$key.GetValueKind($_)
      $value=$key.GetValue($_,$null,[Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
      $data=if($value-is[byte[]]){[Convert]::ToBase64String($value)}elseif($value-is[string[]]){@($value|ForEach-Object {[Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes($_))})}else{[string]$value}
      [pscustomobject]@{Name=$_;Kind=[string]$kind;Data=$data}
    })}
    $key=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey("Software\Talking Quill Tests\$id",$false)
    if($null-eq$key){[pscustomobject]@{Present=$false}|ConvertTo-Json -Compress;exit 0}
    try{
      $children=@($key.GetSubKeyNames()|Sort-Object)
      if($children|Where-Object {$_-ne'RecoveryStateLockV1'}){throw 'registry namespace has an unexpected child'}
      $child=$key.OpenSubKey('RecoveryStateLockV1',$false)
      $childState=$null
      if($null-ne$child){try{if($child.GetSubKeyNames().Count-ne0){throw 'registry child has subkeys'};$childState=[pscustomobject]@{Values=Values($child);Acl=$child.GetAccessControl().GetSecurityDescriptorSddlForm('All')}}finally{$child.Dispose()}}
      [pscustomobject]@{Present=$true;Values=Values($key);Acl=$key.GetAccessControl().GetSecurityDescriptorSddlForm('All');Child=$childState}|ConvertTo-Json -Compress -Depth 6
    }finally{$key.Dispose()}
  `,
    { TQ_NAMESPACE_ID: namespaceId },
  );
}

function deleteExactRegistry(namespaceId, expected) {
  if (registrySnapshot(namespaceId) !== expected)
    throw new Error('test registry changed before deletion');
  const result = powershellText(
    String.raw`
    $id=$env:TQ_NAMESPACE_ID
    $base=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software\Talking Quill Tests',$true)
    if($null-eq$base){exit 0}
    try{
      $namespace=$base.OpenSubKey($id,$true)
      if($null-ne$namespace){
        try{
          if($namespace.GetSubKeyNames() -contains 'RecoveryStateLockV1'){
            $child=$namespace.OpenSubKey('RecoveryStateLockV1',$true)
            try{foreach($name in @($child.GetValueNames())){$child.DeleteValue($name,$true)}}finally{$child.Dispose()}
            $namespace.DeleteSubKey('RecoveryStateLockV1',$false)
          }
          foreach($name in @($namespace.GetValueNames())){$namespace.DeleteValue($name,$true)}
        }finally{$namespace.Dispose()}
        $base.DeleteSubKey($id,$false)
      }
    }finally{$base.Dispose()}
  `,
    { TQ_NAMESPACE_ID: namespaceId },
  );
  void result;
  if (registrySnapshot(namespaceId) !== '{"Present":false}') {
    throw new Error('test registry namespace remains after deletion');
  }
}

function registryNamespaceInventory() {
  return powershellJson(String.raw`
    $path='Registry::HKEY_CURRENT_USER\Software\Talking Quill Tests'
    $key=Get-Item -LiteralPath $path -ErrorAction SilentlyContinue
    $ids=@(Get-ChildItem -LiteralPath $path -ErrorAction SilentlyContinue|ForEach-Object PSChildName)
    $values=if($null-eq$key){@()}else{@($key.GetValueNames())}
    [pscustomobject]@{Ids=@($ids);Values=@($values)}|ConvertTo-Json -Compress
  `);
}

function assertNoRelevantTestProcesses() {
  const matches = powershellJson(
    String.raw`
    $wrapper=[uint32]$env:TQ_WRAPPER_PROCESS_ID
    $serializer=[uint32]$env:TQ_SERIALIZER_PROCESS_ID
    $all=@(Get-CimInstance Win32_Process -ErrorAction Stop)
    $ancestors=@($wrapper,$serializer,$PID)
    $cursor=$all|Where-Object ProcessId-eq$wrapper
    while($null-ne$cursor -and $cursor.ParentProcessId-ne0){
      $ancestors+=[uint32]$cursor.ParentProcessId
      $parent=[uint32]$cursor.ParentProcessId
      $cursor=$all|Where-Object ProcessId-eq$parent
    }
    $matches=@($all|Where-Object {
      $command=[string]$_.CommandLine
      $_.ProcessId-notin$ancestors -and
      $command-notmatch '(?i)run-machine-lock-isolated-tests\.mjs\s+--' -and (
        ($_.Name -match '(?i)^talking[_-]quill.*\.exe$') -or
        ($command -match '(?i)cargo(?:\.exe)?\s+test.+(?:helper|windows-setup)')
      )
    }|Select-Object ProcessId,CreationDate,ExecutablePath,CommandLine)
    [pscustomobject]@{Processes=$matches}|ConvertTo-Json -Compress -Depth 4
  `,
    {
      TQ_WRAPPER_PROCESS_ID: String(process.pid),
      TQ_SERIALIZER_PROCESS_ID: String(serializer.pid),
    },
  ).Processes;
  if (matches.length !== 0) {
    throw new Error(`machine-lock test processes remain alive: ${JSON.stringify(matches)}`);
  }
}

function processIdentity(pid) {
  if (!Number.isInteger(pid) || pid <= 0) return null;
  return powershellJson(
    String.raw`
    $process=Get-CimInstance Win32_Process -Filter "ProcessId=$env:TQ_PROCESS_ID" -ErrorAction SilentlyContinue
    if($null-eq$process){[pscustomobject]@{Pid=[uint32]$env:TQ_PROCESS_ID;CreationDate=$null}|ConvertTo-Json -Compress;exit 0}
    [pscustomobject]@{Pid=$process.ProcessId;CreationDate=[string]$process.CreationDate}|ConvertTo-Json -Compress
  `,
    { TQ_PROCESS_ID: String(pid) },
  );
}

function processIdentityAlive(identity) {
  if (identity === null || identity?.CreationDate == null) return false;
  const current = processIdentity(identity.Pid);
  return current?.CreationDate === identity.CreationDate;
}

function ownedTreeIdentity(path) {
  const result = spawnSync(deleter, ['--identity', path], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`cannot identify owned tree: ${path}`);
  return result.stdout.trim();
}

function pathAcl(path) {
  return powershellText('(Get-Acl -LiteralPath $env:TQ_ACL_PATH -ErrorAction Stop).Sddl', {
    TQ_ACL_PATH: path,
  });
}

function fileIdentity(path) {
  if (!existsSync(path)) throw new Error(`cannot identify missing path: ${path}`);
  const output = powershellText(
    '(& fsutil.exe file queryfileid $env:TQ_IDENTITY_PATH 2>&1 | Out-String).Trim()',
    {
      TQ_IDENTITY_PATH: path,
    },
  );
  const match = /0x([0-9a-f]{32})/iu.exec(output);
  if (!match) throw new Error(`cannot read file identity: ${path}`);
  return match[1].toLowerCase();
}

function isPlainPath(path, directory) {
  if (!existsSync(path)) return false;
  const metadata = lstatSync(path);
  return !metadata.isSymbolicLink() && (directory ? metadata.isDirectory() : metadata.isFile());
}

function namespaceRoot(kind, namespaceId) {
  if (!rootKinds.includes(kind)) throw new Error('unknown machine-lock root kind');
  assertNamespaceId(namespaceId);
  return resolve(stateRoot, kind, namespaceId);
}

function recordPath(recordId) {
  assertNamespaceId(recordId);
  return resolve(recordRoot, `${recordId}.json`);
}

function removeEmptyParents() {
  if (existsSync(recordRoot) && readdirSync(recordRoot).length === 0) rmdirSync(recordRoot);
  for (const kind of rootKinds) {
    const path = resolve(stateRoot, kind);
    if (existsSync(path) && readdirSync(path).length === 0) rmdirSync(path);
  }
  removeEmptyRegistryRoot();
  if (existsSync(stateRoot) && readdirSync(stateRoot).length === 0) rmdirSync(stateRoot);
}

function removeEmptyRegistryRoot() {
  powershellText(String.raw`
    $software=[Microsoft.Win32.Registry]::CurrentUser.OpenSubKey('Software',$true)
    if($null-eq$software){exit 0}
    try{
      $root=$software.OpenSubKey('Talking Quill Tests',$true)
      if($null-ne$root){
        try{if($root.GetSubKeyNames().Count-ne0 -or $root.GetValueNames().Count-ne0){throw 'test registry root is not empty'}}finally{$root.Dispose()}
        $software.DeleteSubKey('Talking Quill Tests',$false)
      }
    }finally{$software.Dispose()}
  `);
}

function fsyncDirectory(path) {
  const result = spawnSync(deleter, ['--flush-directory', path], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`cannot flush cleanup record directory: ${path}`);
}

function productionResidueSnapshot() {
  return powershellText(String.raw`
    $pd=[Environment]::GetFolderPath('CommonApplicationData')
    $roots=@(Get-ChildItem -LiteralPath $pd -Force -ErrorAction Stop|Where-Object {$_.Name -like '.Talking Quill.machine-lock-*' -or $_.Name -like '.Talking Quill.machine-lock-pending-*' -or $_.Name -like '.Talking Quill.machine-lifecycle-retained-*'})
    $items=@()
    foreach($root in $roots){$entries=@($root);if($root.PSIsContainer){$entries+=@(Get-ChildItem -LiteralPath $root.FullName -Force -Recurse -ErrorAction Stop)};foreach($entry in $entries){$items+=[pscustomobject]@{Path=$entry.FullName;Attributes=[string]$entry.Attributes;Length=$entry.Length;Sddl=(Get-Acl -LiteralPath $entry.FullName).Sddl;FileId=(& fsutil.exe file queryfileid $entry.FullName 2>&1|Out-String).Trim();Sha256=if($entry.PSIsContainer){$null}else{(Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash}}}}
    $registry=& reg.exe query 'HKLM\Software\Talking Quill\RecoveryStateLockV1' /s 2>&1|Out-String
    $parentAcl=if(Test-Path 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill'){(Get-Acl 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill').Sddl}else{$null}
    $childAcl=if(Test-Path 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill\RecoveryStateLockV1'){(Get-Acl 'Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill\RecoveryStateLockV1').Sddl}else{$null}
    [pscustomobject]@{Items=@($items|Sort-Object Path);Registry=$registry;ParentAcl=$parentAcl;ChildAcl=$childAcl}|ConvertTo-Json -Compress -Depth 6
  `);
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
  if (result.status !== 0) throw new Error(result.stderr || 'PowerShell operation failed');
  return result.stdout.trim();
}
