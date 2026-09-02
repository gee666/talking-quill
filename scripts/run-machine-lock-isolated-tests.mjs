import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import { createInterface } from 'node:readline';
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

const RECORD_SCHEMA = 3;
const LEGACY_RECORD_SCHEMAS = [1, 2];
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
  const result = await runNamespaceSession(record);
  status = result.code;
  unlinkSync(recordPath(record.recordId));
  fsyncDirectory(recordRoot);
  record = undefined;
  removeEmptyParents();
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
      "$m=[Threading.Mutex]::new($false,'Global\\TalkingQuill.MachineLockTests.V1');try{$acquired=$false;try{$acquired=$m.WaitOne(300000)}catch [Threading.AbandonedMutexException]{$acquired=$true};if(-not $acquired){exit 2};[Console]::Out.WriteLine('ready');[Console]::Out.Flush();[Console]::In.ReadLine()|Out-Null}finally{try{$m.ReleaseMutex()}catch{};$m.Dispose()}",
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
    phase: 'native-creating',
    owner: processIdentity(process.pid),
    child: null,
    supervisor: null,
    projectRootIdentity: fileIdentity(root),
    stateRootIdentity: fileIdentity(stateRoot),
    recordDirectoryIdentity: fileIdentity(recordRoot),
    recordDirectoryAcl: pathAcl(recordRoot),
    registryPath: `HKCU\\Software\\Talking Quill Tests\\${namespaceId}\\RecoveryStateLockV1`,
    creatingRoot: null,
    deletingRoot: null,
    deletedRoots: [],
    registryInventory: null,
    roots: [],
  };
  for (const kind of rootKinds) {
    const path = namespaceRoot(kind, namespaceId);
    mkdirSync(dirname(path), { recursive: true });
    if (existsSync(path)) throw new Error(`machine-lock test root already exists: ${path}`);
    record.roots.push({
      kind,
      parentIdentity: ownedTreeIdentity(dirname(path)),
      identity: null,
      inventory: [],
      ownershipPrefix: `v1.${record.recordId}.${namespaceId}.${kind}.${randomBytes(16).toString('hex')}`,
      bindingNonce: randomBytes(16).toString('hex'),
      bindingFile: `${record.recordId}.${kind}.binding-v1`,
      adsSha256: null,
    });
  }
  writeRecord(record);
  return record;
}

async function runNamespaceSession(record) {
  const request = {
    command,
    namespaceId: record.namespaceId,
    recordPath: recordPath(record.recordId),
    roots: record.roots.map((entry) => ({
      kind: entry.kind,
      path: namespaceRoot(entry.kind, record.namespaceId),
      parentIdentity: entry.parentIdentity,
      ownershipPrefix: entry.ownershipPrefix,
      bindingPath: resolve(recordRoot, entry.bindingFile),
      bindingNonce: entry.bindingNonce,
    })),
  };
  const supervisor = spawn(deleter, ['--namespace-session', JSON.stringify(request)], {
    stdio: ['pipe', 'pipe', 'inherit'],
    windowsHide: true,
    env: { ...process.env, TQ_MACHINE_LOCK_TEST_GUARD_EXE: deleter },
  });
  const exited = new Promise((done, reject) => {
    supervisor.once('error', reject);
    supervisor.once('exit', (code, signal) => done({ code, signal }));
  });
  const lines = createInterface({ input: supervisor.stdout, crlfDelay: Infinity });
  const iterator = lines[Symbol.asyncIterator]();
  const started = await nextNamespaceSessionEvent(iterator);
  if (started === null) return handleNamespaceSessionExit(await exited);
  if (started?.event !== 'started') {
    throw new Error('native namespace supervisor returned an invalid startup event');
  }
  record.supervisor = requiredProcessIdentity(supervisor.pid);
  writeRecord(record);
  supervisor.stdin.write('create\n');
  const setup = await nextNamespaceSessionEvent(iterator);
  if (setup === null) return handleNamespaceSessionExit(await exited);
  if (setup?.event !== 'ready' || !Array.isArray(setup.roots)) {
    throw new Error('native namespace supervisor returned an invalid setup event');
  }
  const byKind = new Map(setup.roots.map((entry) => [entry.kind, entry]));
  for (const rootRecord of record.roots) {
    const created = byKind.get(rootRecord.kind);
    if (
      !created ||
      !/^\d+:\d+$/u.test(created.identity ?? '') ||
      !/^[0-9a-f]{64}$/u.test(created.adsSha256 ?? '')
    ) {
      throw new Error('native namespace supervisor returned an invalid root identity');
    }
    rootRecord.identity = created.identity;
    rootRecord.adsSha256 = created.adsSha256;
  }
  record.creatingRoot = null;
  record.phase = 'roots-created';
  writeRecord(record);
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'roots-created') process.exit(197);
  supervisor.stdin.end('run\n');
  const completed = await nextNamespaceSessionEvent(iterator);
  const result = await exited;
  if (result.code === 197) process.exit(197);
  if (result.signal) throw new Error(`native namespace supervisor ended with ${result.signal}`);
  if (completed === null) {
    throw new Error(`native namespace supervisor exited before teardown: ${result.code}`);
  }
  if (completed?.event !== 'completed' || completed.code !== result.code) {
    throw new Error('native namespace supervisor returned an invalid completion event');
  }
  for (const rootRecord of record.roots) {
    if (existsSync(namespaceRoot(rootRecord.kind, record.namespaceId))) {
      throw new Error(`native namespace supervisor left a root: ${rootRecord.kind}`);
    }
    if (
      existsSync(resolve(recordRoot, rootRecord.bindingFile)) ||
      existsSync(`${resolve(recordRoot, rootRecord.bindingFile)}.intent-v1`)
    ) {
      throw new Error(`native namespace supervisor left a binding: ${rootRecord.kind}`);
    }
  }
  if (registrySnapshot(record.namespaceId).present) {
    throw new Error('native namespace supervisor left the registry namespace');
  }
  return { code: completed.code };
}

async function nextNamespaceSessionEvent(iterator) {
  for (;;) {
    const line = await iterator.next();
    if (line.done) return null;
    if (line.value.startsWith('TQNS:')) return JSON.parse(line.value.slice(5));
    process.stdout.write(`${line.value}\n`);
  }
}

function handleNamespaceSessionExit(result) {
  if (result.code === 197) process.exit(197);
  if (result.signal) throw new Error(`native namespace supervisor ended with ${result.signal}`);
  throw new Error(`native namespace supervisor failed during setup: ${result.code}`);
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
  recoverNativeRecordTemps();
  const files = readdirSync(recordRoot).sort();
  for (const name of files) {
    if (
      !/^[0-9a-f]{32}\.json$/u.test(name) &&
      !/^[0-9a-f]{32}\.[a-z-]+\.binding-v1(?:\.intent-v1)?$/u.test(name)
    ) {
      throw new Error(`unknown machine-lock cleanup record: ${name}`);
    }
  }
  const records = files
    .filter((name) => name.endsWith('.json'))
    .map((name) => readRecord(resolve(recordRoot, name)));
  const expectedBindings = new Set(
    records.flatMap((record) =>
      record.roots.flatMap((entry) => [entry.bindingFile, `${entry.bindingFile}.intent-v1`]),
    ),
  );
  for (const name of files.filter(
    (entry) => entry.endsWith('.binding-v1') || entry.endsWith('.intent-v1'),
  )) {
    if (!expectedBindings.has(name))
      throw new Error(`orphan machine-lock cleanup binding: ${name}`);
  }
  const ids = new Set();
  for (const value of records) {
    if (ids.has(value.namespaceId))
      throw new Error('duplicate machine-lock cleanup namespace record');
    ids.add(value.namespaceId);
  }
  assertNoUnknownNamespaces(ids);
  for (const value of records) {
    if (
      processIdentityAlive(value.owner) ||
      processIdentityAlive(value.child) ||
      processIdentityAlive(value.supervisor)
    ) {
      throw new Error(`machine-lock cleanup record is owned by a live process: ${value.recordId}`);
    }
    cleanupRecord(value, deleter, false);
  }
  removeEmptyParents();
}

function recoverNativeRecordTemps() {
  for (const name of readdirSync(recordRoot).sort()) {
    const match = /^([0-9a-f]{32})\.native-tmp$/u.exec(name);
    if (!match) continue;
    const temporary = resolve(recordRoot, name);
    const destination = recordPath(match[1]);
    if (!existsSync(destination)) {
      throw new Error(`native cleanup-record temporary has no main record: ${name}`);
    }
    assertProtectedRecord(destination);
    const consumed = spawnSync(deleter, ['--consume-record-temp', temporary], {
      encoding: 'utf8',
      windowsHide: true,
    });
    if (consumed.status !== 0) {
      throw new Error(`cannot consume native cleanup-record temporary: ${name}`);
    }
    try {
      const candidate = JSON.parse(consumed.stdout);
      validateRecord(candidate);
      if (candidate.recordId !== match[1]) {
        throw new Error('native cleanup-record temporary identity changed');
      }
      writeRecord(candidate);
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error;
    }
  }
}

function cleanupRecord(record, deleter, ownerMayBeCurrent) {
  validateRecord(record);
  if (!ownerMayBeCurrent && processIdentityAlive(record.owner)) {
    throw new Error('cannot recover a live machine-lock test wrapper');
  }
  if (processIdentityAlive(record.child)) {
    throw new Error('cannot clean a live machine-lock test child');
  }
  if (processIdentityAlive(record.supervisor)) {
    throw new Error('cannot clean a live machine-lock namespace supervisor');
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
  if (record.roots.length !== rootKinds.length) record.partialCreation = true;
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
      if (
        rootRecord.identity === null &&
        (record.creatingRoot === rootRecord.kind ||
          (record.schemaVersion === RECORD_SCHEMA && record.phase === 'native-creating'))
      ) {
        deleteCreationArtifacts(rootRecord);
        record.deletedRoots.push(rootRecord.kind);
        record.creatingRoot = null;
        continue;
      }
      if (
        record.deletedRoots.includes(rootRecord.kind) ||
        record.deletingRoot === rootRecord.kind
      ) {
        if (rootRecord.identity === null) deleteCreationArtifacts(rootRecord);
        else deleteRootBinding(rootRecord);
        if (!record.deletedRoots.includes(rootRecord.kind))
          record.deletedRoots.push(rootRecord.kind);
        record.deletingRoot = null;
        continue;
      }
      throw new Error(`recorded machine-lock root is missing: ${path}`);
    }
    if (
      rootRecord.identity === null &&
      (record.creatingRoot === rootRecord.kind ||
        (record.schemaVersion === RECORD_SCHEMA && record.phase === 'native-creating'))
    ) {
      removeInterruptedNamespaceRoot(path, rootRecord);
      record.deletedRoots.push(rootRecord.kind);
      record.creatingRoot = null;
      continue;
    }
    if (rootRecord.identity === null) {
      throw new Error(`recorded machine-lock root identity is absent: ${path}`);
    }
    verifyRootBinding(path, rootRecord);
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
    deleteRootBinding(rootRecord);
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
  if (registryNow.present) {
    const expected =
      record.schemaVersion === RECORD_SCHEMA && record.phase === 'deleting-registry'
        ? registryNow
        : registryInventory;
    if (
      record.schemaVersion === RECORD_SCHEMA &&
      record.phase === 'deleting-registry' &&
      !registryInventorySubset(registryNow, registryInventory)
    ) {
      throw new Error('partially deleted registry namespace is not an exact recorded subset');
    }
    deleteExactRegistry(record.namespaceId, expected);
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

function removeInterruptedNamespaceRoot(path, rootRecord) {
  const result = spawnSync(
    deleter,
    [
      '--remove-interrupted-root',
      path,
      rootRecord.ownershipPrefix,
      resolve(recordRoot, rootRecord.bindingFile),
      rootRecord.bindingNonce,
    ],
    {
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (result.status !== 0) {
    throw new Error(`interrupted protected root is not an exact authenticated orphan: ${path}`);
  }
}

function deleteCreationArtifacts(rootRecord) {
  const result = spawnSync(
    deleter,
    [
      '--delete-creation-artifacts',
      resolve(recordRoot, rootRecord.bindingFile),
      rootRecord.ownershipPrefix,
      rootRecord.bindingNonce,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status !== 0) throw new Error('interrupted creation artifact deletion failed');
}

function verifyRootBinding(path, rootRecord) {
  const result = spawnSync(
    deleter,
    [
      '--verify-root-binding',
      path,
      rootRecord.ownershipPrefix,
      resolve(recordRoot, rootRecord.bindingFile),
      rootRecord.bindingNonce,
      rootRecord.identity,
      rootRecord.adsSha256,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status !== 0) throw new Error(`protected root binding changed: ${path}`);
}

function deleteRootBinding(rootRecord) {
  const path = resolve(recordRoot, rootRecord.bindingFile);
  if (!existsSync(path) && !existsSync(`${path}.intent-v1`)) return;
  const result = spawnSync(
    deleter,
    [
      '--delete-root-binding',
      path,
      rootRecord.ownershipPrefix,
      rootRecord.bindingNonce,
      rootRecord.identity,
      rootRecord.adsSha256,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status === 197) process.exit(197);
  if (result.status !== 0) throw new Error(`protected root binding deletion failed: ${path}`);
}

function registryInventorySubset(actual, expected) {
  if (!actual?.present || !expected?.present) return false;
  const keySubset = (current, sealed) => {
    if (current === null) return true;
    if (sealed === null || current.security !== sealed.security) return false;
    return (
      current.subkeys.every((name) => sealed.subkeys.includes(name)) &&
      current.values.every((value) =>
        sealed.values.some(
          (entry) =>
            entry.name === value.name &&
            entry.valueType === value.valueType &&
            entry.dataHex === value.dataHex,
        ),
      )
    );
  };
  return (
    keySubset(actual.namespace, expected.namespace) && keySubset(actual.recovery, expected.recovery)
  );
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
  if (![...LEGACY_RECORD_SCHEMAS, RECORD_SCHEMA].includes(record?.schemaVersion)) {
    throw new Error('cleanup record schema is invalid');
  }
  assertNamespaceId(record.namespaceId);
  assertNamespaceId(record.recordId);
  const validProcessIdentity = (identity) =>
    Number.isInteger(identity?.Pid) &&
    identity.Pid > 0 &&
    typeof identity.CreationDate === 'string' &&
    identity.CreationDate.length > 0;
  if (
    !validProcessIdentity(record.owner) ||
    (record.child !== null && !validProcessIdentity(record.child))
  ) {
    throw new Error('cleanup record process identity is invalid');
  }
  if (
    record.schemaVersion === RECORD_SCHEMA &&
    record.phase !== 'native-creating' &&
    !validProcessIdentity(record.supervisor)
  ) {
    throw new Error('cleanup record supervisor identity is invalid');
  }
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
  for (const entry of record.roots) {
    const prefix = `v1.${record.recordId}.${record.namespaceId}.${entry.kind}.`;
    if (
      typeof entry.ownershipPrefix !== 'string' ||
      !entry.ownershipPrefix.startsWith(prefix) ||
      !/^[0-9a-f]{32}$/u.test(entry.ownershipPrefix.slice(prefix.length)) ||
      (record.schemaVersion === RECORD_SCHEMA && !/^\d+:\d+$/u.test(entry.parentIdentity ?? '')) ||
      (entry.parentIdentity !== undefined && !/^\d+:\d+$/u.test(entry.parentIdentity)) ||
      !/^[0-9a-f]{32}$/u.test(entry.bindingNonce ?? '') ||
      entry.bindingFile !== `${record.recordId}.${entry.kind}.binding-v1` ||
      (entry.adsSha256 !== null && !/^[0-9a-f]{64}$/u.test(entry.adsSha256)) ||
      (entry.identity === null && entry.adsSha256 !== null) ||
      (entry.identity !== null && entry.adsSha256 === null) ||
      (entry.identity === null &&
        record.schemaVersion !== RECORD_SCHEMA &&
        record.creatingRoot !== entry.kind &&
        !record.deletedRoots.includes(entry.kind)) ||
      (entry.identity === null &&
        record.schemaVersion === RECORD_SCHEMA &&
        record.phase !== 'native-creating' &&
        !record.deletedRoots.includes(entry.kind)) ||
      (entry.identity !== null && !/^\d+:\d+$/u.test(entry.identity))
    ) {
      throw new Error('cleanup record root ownership is invalid');
    }
  }
  if (
    !['prepared', 'native-creating'].includes(record.phase) &&
    record.partialCreation !== true &&
    kinds.length !== rootKinds.length
  ) {
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
  const result = spawnSync(deleter, ['--registry-inventory', namespaceId], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error('native test registry inventory failed');
  return JSON.parse(result.stdout);
}

function deleteExactRegistry(namespaceId, expected) {
  const result = spawnSync(deleter, ['--registry-delete-exact', namespaceId], {
    input: JSON.stringify(expected),
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error('native exact test registry deletion failed');
}

function registryNamespaceInventory() {
  const result = spawnSync(deleter, ['--registry-root-inventory'], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error('native test registry root inventory failed');
  const inventory = JSON.parse(result.stdout);
  return {
    Ids: inventory?.subkeys ?? [],
    Values: inventory?.values ?? [],
  };
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

function requiredProcessIdentity(pid) {
  for (let attempt = 0; attempt < 20; attempt += 1) {
    const identity = processIdentity(pid);
    if (identity?.CreationDate != null) return identity;
    spawnSync(
      'powershell.exe',
      ['-NoProfile', '-NonInteractive', '-Command', 'Start-Sleep -Milliseconds 50'],
      {
        windowsHide: true,
      },
    );
  }
  throw new Error('cannot authenticate native namespace supervisor identity');
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
  const result = spawnSync(deleter, ['--registry-delete-empty-root'], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error('native empty test registry root deletion failed');
}

function fsyncDirectory(path) {
  const result = spawnSync(deleter, ['--flush-directory', path], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (result.status !== 0) throw new Error(`cannot flush cleanup record directory: ${path}`);
}

function productionResidueSnapshot() {
  const streamInventoryHelper = deleter.replaceAll("'", "''");
  return powershellText(String.raw`
    $native='${streamInventoryHelper}'
    $pd=[Environment]::GetFolderPath('CommonApplicationData')
    $roots=@(Get-ChildItem -LiteralPath $pd -Force -ErrorAction Stop|Where-Object {$_.Name -like '.Talking Quill.machine-lock-*' -or $_.Name -like '.Talking Quill.machine-lock-pending-*' -or $_.Name -like '.Talking Quill.machine-lifecycle-retained-*'})
    $items=@()
    foreach($root in $roots){
      $entries=@($root)
      if($root.PSIsContainer){$entries+=@(Get-ChildItem -LiteralPath $root.FullName -Force -Recurse -ErrorAction Stop)}
      foreach($entry in $entries){
        $streamJson=& $native '--stream-inventory' $entry.FullName
        if($LASTEXITCODE-ne0){throw "native NTFS stream inventory failed: $($entry.FullName)"}
        $streams=@($streamJson|ConvertFrom-Json -ErrorAction Stop)
        $items+=[pscustomobject]@{
          Path=$entry.FullName
          Attributes=[string]$entry.Attributes
          Length=$entry.Length
          Sddl=(Get-Acl -LiteralPath $entry.FullName).Sddl
          FileId=(& fsutil.exe file queryfileid $entry.FullName 2>&1|Out-String).Trim()
          Sha256=if($entry.PSIsContainer){$null}else{(Get-FileHash -LiteralPath $entry.FullName -Algorithm SHA256).Hash}
          Streams=$streams
        }
      }
    }
    $registryPath='Registry::HKEY_LOCAL_MACHINE\Software\Talking Quill'
    $registry=& reg.exe query 'HKLM\Software\Talking Quill' /s 2>&1|Out-String
    $registryKeys=@()
    if(Test-Path -LiteralPath $registryPath){
      $keys=@(Get-Item -LiteralPath $registryPath -ErrorAction Stop)+@(Get-ChildItem -LiteralPath $registryPath -Recurse -ErrorAction Stop)
      $registryKeys=@($keys|ForEach-Object {
        [pscustomobject]@{
          Path=$_.Name
          Acl=$_.GetAccessControl().GetSecurityDescriptorSddlForm('All')
          Subkeys=@($_.GetSubKeyNames()|Sort-Object)
          Values=@($_.GetValueNames()|Sort-Object)
        }
      }|Sort-Object Path)
    }
    [pscustomobject]@{Items=@($items|Sort-Object Path);Registry=$registry;RegistryKeys=$registryKeys}|ConvertTo-Json -Compress -Depth 8
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
