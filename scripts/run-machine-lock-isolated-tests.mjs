import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import { createInterface } from 'node:readline';
import {
  closeSync,
  existsSync,
  lstatSync,
  mkdirSync,
  openSync,
  readFileSync,
  readSync,
  readdirSync,
  rmdirSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { dirname, relative, resolve } from 'node:path';

const RECORD_SCHEMA = 5;
const NATIVE_SESSION_SCHEMA = 3;
const LEGACY_RECORD_SCHEMAS = [1, 2, 3, 4];
const root = resolve(import.meta.dirname, '..');
const stateRoot = resolve(root, 'tmp', 'machine-lock-tests');
const recordRoot = resolve(stateRoot, '.cleanup-records-v1');
const legacyEvidenceRoot = resolve(root, 'tmp', 'machine-lock-log-evidence-v1');
const rootKinds = ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit'];
const separator = process.argv.indexOf('--');
if (separator === -1 || separator === process.argv.length - 1) {
  throw new Error('Usage: run-machine-lock-isolated-tests.mjs -- <command>');
}
const command = process.argv.slice(separator + 1).join(' ');

class SupervisorFailure extends Error {
  constructor(code, message) {
    super(message);
    this.code = code;
  }
}

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
let primaryFailure;
const finalFailures = [];
try {
  recoverRecordedNamespaces(deleter);
  assertNoUnknownNamespaces();
  productionBefore = productionResidueSnapshot();
  record = prepareNamespace(randomBytes(16).toString('hex'));
  process.env.TQ_MACHINE_LOCK_TEST_NAMESPACE_ID = record.namespaceId;
  const result = await runNamespaceSession(record);
  status = result.code;
  deleteCleanupRecord(record.recordId);
  record = undefined;
  removeEmptyParents();
} catch (error) {
  primaryFailure = error;
  status = error instanceof SupervisorFailure && error.code === 197 ? 197 : 1;
  if (status === 197) {
    delete process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER;
    await supervisorFailureTestPause();
  }
} finally {
  try {
    if (record !== undefined) {
      assertSupervisorExited(record);
      recoverNativeRecordBackups();
      recoverNativeRecordTemps();
      if (existsSync(recordPath(record.recordId))) {
        record = reloadProtectedRecord(record.recordId);
        cleanupRecord(record, deleter, true);
      } else {
        assertRetiredRecordNamespaceIsGone(record);
        record = undefined;
        removeEmptyParents();
      }
    }
  } catch (error) {
    finalFailures.push(error);
  }
  try {
    recoverRecordedNamespaces(deleter, true);
  } catch (error) {
    finalFailures.push(error);
  }
  try {
    assertNoUnknownNamespaces();
  } catch (error) {
    finalFailures.push(error);
  }
  try {
    if (productionBefore !== undefined && productionResidueSnapshot() !== productionBefore) {
      finalFailures.push(new Error('isolated tests changed production machine-lock residue'));
    }
  } catch (error) {
    finalFailures.push(error);
  } finally {
    serializer.stdin.end('\n');
  }
}
if (
  primaryFailure &&
  !(primaryFailure instanceof SupervisorFailure && primaryFailure.code === 197)
) {
  finalFailures.unshift(primaryFailure);
}
if (finalFailures.length > 0) throw new AggregateError(finalFailures, 'namespace wrapper failed');
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
  const recordId = randomBytes(16).toString('hex');
  const record = {
    schemaVersion: RECORD_SCHEMA,
    revision: 0,
    recordId,
    namespaceId,
    phase: 'native-creating',
    owner: processIdentity(process.pid),
    child: null,
    supervisor: null,
    controlNonce: randomBytes(16).toString('hex'),
    childStdoutLogFile: `${recordId}.stdout.log`,
    childStderrLogFile: `${recordId}.stderr.log`,
    logsRetired: false,
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
    const parent = dirname(path);
    if (!existsSync(parent)) {
      mkdirSync(parent);
      fsyncDirectory(parent);
      fsyncDirectory(dirname(parent));
    }
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
  const childStdoutLogPath = resolve(recordRoot, record.childStdoutLogFile);
  const childStderrLogPath = resolve(recordRoot, record.childStderrLogFile);
  if (existsSync(childStdoutLogPath) || existsSync(childStderrLogPath)) {
    throw new Error('native namespace child log already exists');
  }
  const request = {
    command,
    namespaceId: record.namespaceId,
    recordPath: recordPath(record.recordId),
    childStdoutLogPath,
    childStderrLogPath,
    controlNonce: record.controlNonce,
    probeControlHandle: process.env.TQ_MACHINE_LOCK_TEST_PROBE_CONTROL_HANDLE === '1',
    roots: record.roots.map((entry) => ({
      kind: entry.kind,
      path: namespaceRoot(entry.kind, record.namespaceId),
      parentIdentity: entry.parentIdentity,
      ownershipPrefix: entry.ownershipPrefix,
      bindingPath: resolve(recordRoot, entry.bindingFile),
      bindingNonce: entry.bindingNonce,
    })),
  };
  const supervisor = spawn(deleter, ['--namespace-session'], {
    stdio: ['pipe', 'pipe', 'inherit'],
    windowsHide: true,
    env: { ...process.env, TQ_MACHINE_LOCK_TEST_GUARD_EXE: deleter },
  });
  supervisor.stdin.write(`${JSON.stringify(request)}\n`);
  const exited = new Promise((done, reject) => {
    supervisor.once('error', reject);
    supervisor.once('exit', (code, signal) => done({ code, signal }));
  });
  let logsDrained = false;
  try {
    const lines = createInterface({ input: supervisor.stdout, crlfDelay: Infinity });
    const iterator = lines[Symbol.asyncIterator]();
    const started = await nextNamespaceSessionEvent(iterator, record.controlNonce);
    if (started === null) return handleNamespaceSessionExit(await exited, record);
    if (started?.event !== 'started') {
      throw new Error('native namespace supervisor returned an invalid startup event');
    }
    record.supervisor = requiredProcessIdentity(supervisor.pid);
    writeRecord(record);
    supervisor.stdin.write('create\n');
    const setup = await nextNamespaceSessionEvent(iterator, record.controlNonce);
    if (setup === null) return handleNamespaceSessionExit(await exited, record);
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
    supervisor.stdin.write('run\n');
    const completed = await nextNamespaceSessionEvent(iterator, record.controlNonce);
    if (completed === null) {
      const failed = await exited;
      assertSupervisorExited(record);
      throw new SupervisorFailure(
        failed.code,
        `native namespace supervisor exited before authenticated teardown: ${failed.code}`,
      );
    }
    if (completed.event !== 'completed') {
      throw new Error('native namespace supervisor returned an invalid completion event');
    }
    await drainChildLog(childStdoutLogPath, process.stdout);
    await drainChildLog(childStderrLogPath, process.stderr);
    logsDrained = true;
    supervisor.stdin.end('drained\n');
    const result = await exited;
    if (result.signal) throw new Error(`native namespace supervisor ended with ${result.signal}`);
    if (result.code !== 0 || completed?.event !== 'completed') {
      assertSupervisorExited(record);
      throw new SupervisorFailure(
        result.code,
        `native namespace supervisor exited before authenticated teardown: ${result.code}`,
      );
    }
    if (!Number.isInteger(completed.childExitCode) || completed.childExitCode < 0) {
      throw new Error('native namespace supervisor returned an invalid child exit code');
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
    return { code: completed.childExitCode };
  } catch (error) {
    supervisor.stdin.destroy();
    if (supervisor.exitCode === null && supervisor.signalCode === null) supervisor.kill();
    await exited.catch(() => undefined);
    assertSupervisorExited(record);
    throw error;
  } finally {
    if (logsDrained) {
      cleanupChildLog(
        childStdoutLogPath,
        record.recordId,
        'stdout',
        record.recordDirectoryIdentity,
      );
      cleanupChildLog(
        childStderrLogPath,
        record.recordId,
        'stderr',
        record.recordDirectoryIdentity,
      );
      record.logsRetired = true;
      writeRecord(record);
    }
  }
}

async function supervisorFailureTestPause() {
  const path = process.env.TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE;
  if (!path) return;
  writeFileSync(`${path}.ready`, 'ready\n', { flag: 'wx' });
  while (!existsSync(`${path}.continue`)) {
    await new Promise((done) => setTimeout(done, 10));
  }
}

function recoveredLogPlans(record) {
  const plans = [];
  if (record.childStdoutLogFile !== undefined) {
    plans.push({ stream: 'stdout', source: record.childStdoutLogFile });
  }
  if (record.childStderrLogFile !== undefined) {
    plans.push({ stream: 'stderr', source: record.childStderrLogFile });
  }
  if (record.legacyChildLogFile !== undefined || record.childLogFile !== undefined) {
    plans.push({ stream: 'combined', source: record.legacyChildLogFile ?? record.childLogFile });
  }
  return plans;
}

function recoveredLogCrashAt(phase) {
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === phase) {
    throw new SupervisorFailure(197, `injected crash at ${phase}`);
  }
}

function emitRecoveredLog(frame) {
  try {
    if (process.env.TQ_MACHINE_LOCK_TEST_DIAGNOSTIC_WRITE_FAIL === '1') {
      throw new Error('injected recovered-log diagnostic write failure');
    }
    writeFileSync(process.stderr.fd, `TQ_MACHINE_LOCK_RECOVERED_LOG:${JSON.stringify(frame)}\n`);
  } catch {
    // Recovered output is diagnostic only. Teardown must continue.
  }
}

function emitStreamedRecoveredLog(recordId, stream, log, source = 'record-log') {
  emitRecoveredLog({
    version: 2,
    source,
    recordId,
    stream,
    hash: log.sha256,
    byteLength: log.byteLength,
    contentPrefix: log.contentPrefix,
    prefixByteLength: log.prefixByteLength,
    truncated: log.truncated,
  });
}

function retireRecoveredLogs(record) {
  if (record.logsRetired === true) return;
  for (const plan of recoveredLogPlans(record)) {
    const path = resolve(recordRoot, plan.source);
    const inspected = spawnSync(
      deleter,
      ['--inspect-cleanup-log', path, record.recordId, plan.stream, record.recordDirectoryIdentity],
      {
        encoding: 'utf8',
        windowsHide: true,
      },
    );
    if (![0, 3].includes(inspected.status)) {
      throw new Error(`recovered ${plan.stream} log authentication failed`);
    }
    const log = inspected.status === 0 ? JSON.parse(inspected.stdout) : null;
    if (log !== null) {
      recoveredLogCrashAt(`recovered-log-authenticated:${plan.stream}`);
      // Emission precedes deletion. A crash may repeat this frame; consumers deduplicate by
      // recordId, stream, and hash.
      emitStreamedRecoveredLog(record.recordId, plan.stream, log);
      recoveredLogCrashAt(`recovered-log-emitted:${plan.stream}`);
    }
    const deleted = spawnSync(
      deleter,
      [
        '--delete-cleanup-log',
        path,
        record.recordId,
        log?.sha256 ?? '-',
        log?.byteLength?.toString() ?? '-',
        log?.fileIdentity ?? '-',
        plan.stream,
        record.recordDirectoryIdentity,
      ],
      {
        encoding: 'utf8',
        windowsHide: true,
        env: process.env,
      },
    );
    if (deleted.status === 197) {
      throw new SupervisorFailure(197, `injected ${plan.stream} log retirement crash`);
    }
    if (deleted.status !== 0) {
      throw new Error(`recovered ${plan.stream} log deletion failed: ${deleted.stderr.trim()}`);
    }
  }
  recoveredLogCrashAt('recovered-logs-retired');
  record.logsRetired = true;
  writeRecord(record);
  recoveredLogCrashAt('logs-retired-record-published');
}

function inspectLegacyEvidenceRoot() {
  if (!existsSync(legacyEvidenceRoot)) return null;
  if (!isPlainPath(legacyEvidenceRoot, true)) {
    throw new Error('legacy recovered-log evidence root is not plain');
  }
  const inspected = spawnSync(deleter, ['--inspect-legacy-evidence-root', legacyEvidenceRoot], {
    encoding: 'utf8',
    windowsHide: true,
  });
  if (inspected.status !== 0) {
    throw new Error(`legacy evidence root authentication failed: ${inspected.stderr.trim()}`);
  }
  const root = JSON.parse(inspected.stdout);
  if (!Array.isArray(root.names)) {
    throw new Error('legacy evidence root inventory is invalid');
  }
  for (const name of root.names) {
    if (!/^[0-9a-f]{32}\.(?:stdout|stderr|combined)\.evidence-v1(?:\.pending-v1)?$/u.test(name)) {
      throw new Error(`unknown legacy recovered-log evidence: ${name}`);
    }
  }
  return root;
}

function inspectLegacyEvidenceArtifact(rootIdentity, recordId, stream, pending) {
  const suffix = pending ? '.pending-v1' : '';
  const path = resolve(legacyEvidenceRoot, `${recordId}.${stream}.evidence-v1${suffix}`);
  const inspected = spawnSync(
    deleter,
    [
      '--inspect-legacy-evidence',
      path,
      recordId,
      stream,
      pending ? 'pending' : 'final',
      rootIdentity,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (![0, 3].includes(inspected.status)) {
    throw new Error(`legacy ${stream} evidence authentication failed: ${inspected.stderr.trim()}`);
  }
  return { path, log: inspected.status === 0 ? JSON.parse(inspected.stdout) : null };
}

function deleteLegacyEvidenceArtifact(rootIdentity, recordId, stream, pending, path, log) {
  const deleted = spawnSync(
    deleter,
    [
      '--delete-legacy-evidence',
      path,
      recordId,
      stream,
      pending ? 'pending' : 'final',
      rootIdentity,
      log?.sha256 ?? '-',
      log?.byteLength?.toString() ?? '-',
      log?.fileIdentity ?? '-',
    ],
    { encoding: 'utf8', windowsHide: true, env: process.env },
  );
  if (deleted.status === 197) {
    throw new SupervisorFailure(197, `injected legacy ${stream} evidence retirement crash`);
  }
  if (deleted.status !== 0) {
    throw new Error(`legacy ${stream} evidence deletion failed: ${deleted.stderr.trim()}`);
  }
}

function removeLegacyEvidenceRootIfEmpty(rootIdentity) {
  if (!existsSync(legacyEvidenceRoot) || readdirSync(legacyEvidenceRoot).length !== 0) return;
  const removed = spawnSync(
    deleter,
    ['--remove-empty-legacy-evidence-root', legacyEvidenceRoot, rootIdentity],
    { encoding: 'utf8', windowsHide: true, env: process.env },
  );
  if (removed.status === 197) {
    throw new SupervisorFailure(197, 'injected legacy evidence root retirement crash');
  }
  if (removed.status !== 0) {
    throw new Error(`legacy evidence root retirement failed: ${removed.stderr.trim()}`);
  }
}

function authenticateLegacyEvidenceRecord(record, allowMissing) {
  const root = inspectLegacyEvidenceRoot();
  if (
    root === null ||
    root.identity !== record.evidenceDirectoryIdentity ||
    pathAcl(legacyEvidenceRoot) !== record.evidenceDirectoryAcl
  ) {
    throw new Error('legacy evidence root identity or ACL changed');
  }
  const expected = new Set(
    record.recoveredLogEvidence.filter((entry) => entry.present).map((entry) => entry.fileName),
  );
  for (const name of root.names) {
    if (name.startsWith(`${record.recordId}.`) && !expected.has(name)) {
      throw new Error(`unknown legacy evidence for cleanup record: ${name}`);
    }
  }
  const artifacts = [];
  for (const entry of record.recoveredLogEvidence) {
    const inspected = inspectLegacyEvidenceArtifact(
      root.identity,
      record.recordId,
      entry.channel,
      false,
    );
    if (entry.present) {
      if (
        (inspected.log === null && !allowMissing) ||
        (inspected.log !== null &&
          (inspected.log.byteLength !== entry.byteLength || inspected.log.sha256 !== entry.sha256))
      ) {
        throw new Error(`legacy recovered-log evidence changed: ${entry.fileName}`);
      }
      if (inspected.log !== null) {
        artifacts.push({
          channel: entry.channel,
          fileName: entry.fileName,
          byteLength: inspected.log.byteLength,
          sha256: inspected.log.sha256,
          fileIdentity: inspected.log.fileIdentity,
          status: 'pending',
        });
      }
    } else if (inspected.log !== null) {
      throw new Error(`unexpected legacy recovered-log evidence: ${entry.fileName}`);
    }
  }
  return { ...root, artifacts };
}

function retireLegacyEvidence(record) {
  const root = inspectLegacyEvidenceRoot();
  if (
    root === null ||
    root.identity !== record.evidenceDirectoryIdentity ||
    root.identity !== record.legacyEvidenceMigration.rootIdentity ||
    pathAcl(legacyEvidenceRoot) !== record.evidenceDirectoryAcl
  ) {
    throw new Error('legacy evidence migration root changed');
  }
  const expectedNames = new Set(
    record.recoveredLogEvidence.filter((entry) => entry.present).map((entry) => entry.fileName),
  );
  for (const name of root.names) {
    if (name.startsWith(`${record.recordId}.`) && !expectedNames.has(name)) {
      throw new Error(`unknown legacy evidence for cleanup record: ${name}`);
    }
  }
  const remainingNames = new Set(root.names);
  for (const artifact of record.legacyEvidenceMigration.artifacts) {
    const currentRoot = inspectLegacyEvidenceRoot();
    if (
      currentRoot === null ||
      currentRoot.identity !== root.identity ||
      JSON.stringify(currentRoot.names) !== JSON.stringify([...remainingNames].sort())
    ) {
      throw new Error('legacy evidence migration inventory changed');
    }
    const inspected = inspectLegacyEvidenceArtifact(
      root.identity,
      record.recordId,
      artifact.channel,
      false,
    );
    if (
      inspected.log !== null &&
      (inspected.log.byteLength !== artifact.byteLength ||
        inspected.log.sha256 !== artifact.sha256 ||
        inspected.log.fileIdentity !== artifact.fileIdentity)
    ) {
      throw new Error(`legacy evidence migration artifact changed: ${artifact.fileName}`);
    }
    if (artifact.status === 'pending' && inspected.log === null) {
      throw new Error(`legacy evidence disappeared before retirement intent: ${artifact.fileName}`);
    }
    if (artifact.status === 'retired' && inspected.log !== null) {
      throw new Error(`retired legacy evidence reappeared: ${artifact.fileName}`);
    }
    if (artifact.status === 'retired') continue;
    if (inspected.log !== null) {
      recoveredLogCrashAt(`legacy-evidence-authenticated:${artifact.channel}`);
      emitStreamedRecoveredLog(record.recordId, artifact.channel, inspected.log, 'legacy-evidence');
      recoveredLogCrashAt(`legacy-evidence-emitted:${artifact.channel}`);
    }
    if (artifact.status === 'pending') {
      artifact.status = 'retiring';
      writeRecord(record);
      recoveredLogCrashAt(`legacy-evidence-retirement-intent:${artifact.channel}`);
    }
    deleteLegacyEvidenceArtifact(
      root.identity,
      record.recordId,
      artifact.channel,
      false,
      inspected.path,
      inspected.log,
    );
    remainingNames.delete(artifact.fileName);
    artifact.status = 'retired';
    writeRecord(record);
    recoveredLogCrashAt(`legacy-evidence-retirement-recorded:${artifact.channel}`);
  }
  const finalRoot = inspectLegacyEvidenceRoot();
  if (
    finalRoot === null ||
    finalRoot.identity !== root.identity ||
    JSON.stringify(finalRoot.names) !== JSON.stringify([...remainingNames].sort()) ||
    finalRoot.names.some((name) => name.startsWith(`${record.recordId}.`))
  ) {
    throw new Error('legacy evidence retirement inventory is not empty');
  }
  record.phase = record.legacyEvidenceMigration.effectivePhase;
  record.logsRetired = true;
  delete record.logsPreserved;
  delete record.logsPreservedFromPhase;
  delete record.recoveredLogEvidence;
  delete record.evidenceDirectoryIdentity;
  delete record.evidenceDirectoryAcl;
  delete record.legacyEvidenceMigration;
  writeRecord(record);
  recoveredLogCrashAt('legacy-evidence-record-published');
  removeLegacyEvidenceRootIfEmpty(root.identity);
}

function retireOrphanLegacyEvidence() {
  const root = inspectLegacyEvidenceRoot();
  if (root === null) return;
  const remainingNames = new Set(root.names);
  const groups = new Map();
  for (const name of root.names) {
    const match = /^([0-9a-f]{32})\.(stdout|stderr|combined)\.evidence-v1(\.pending-v1)?$/u.exec(
      name,
    );
    const [, recordId, stream, pendingSuffix] = match;
    const key = `${recordId}:${stream}`;
    const group = groups.get(key) ?? { recordId, stream, final: null, pending: null };
    group[pendingSuffix ? 'pending' : 'final'] = name;
    groups.set(key, group);
  }
  for (const group of groups.values()) {
    const current = inspectLegacyEvidenceRoot();
    if (
      current === null ||
      current.identity !== root.identity ||
      JSON.stringify(current.names) !== JSON.stringify([...remainingNames].sort())
    ) {
      throw new Error('legacy orphan evidence inventory changed');
    }
    const final = inspectLegacyEvidenceArtifact(root.identity, group.recordId, group.stream, false);
    const pending = inspectLegacyEvidenceArtifact(
      root.identity,
      group.recordId,
      group.stream,
      true,
    );
    const diagnostic = final.log ?? pending.log;
    if (
      final.log !== null &&
      pending.log !== null &&
      (final.log.sha256 !== pending.log.sha256 || final.log.byteLength !== pending.log.byteLength)
    ) {
      throw new Error('conflicting final and pending legacy recovered-log evidence');
    }
    if (diagnostic !== null) {
      emitStreamedRecoveredLog(group.recordId, group.stream, diagnostic, 'orphan-evidence');
    }
    if (final.log !== null) {
      deleteLegacyEvidenceArtifact(
        root.identity,
        group.recordId,
        group.stream,
        false,
        final.path,
        final.log,
      );
      remainingNames.delete(`${group.recordId}.${group.stream}.evidence-v1`);
    }
    if (pending.log !== null) {
      deleteLegacyEvidenceArtifact(
        root.identity,
        group.recordId,
        group.stream,
        true,
        pending.path,
        pending.log,
      );
      remainingNames.delete(`${group.recordId}.${group.stream}.evidence-v1.pending-v1`);
    }
  }
  const final = inspectLegacyEvidenceRoot();
  if (
    final === null ||
    final.identity !== root.identity ||
    JSON.stringify(final.names) !== JSON.stringify([...remainingNames].sort()) ||
    final.names.length !== 0
  ) {
    throw new Error('legacy orphan evidence retirement is incomplete');
  }
  removeLegacyEvidenceRootIfEmpty(root.identity);
}

async function drainChildLog(path, destination) {
  if (!existsSync(path)) return;
  const handle = openSync(path, 'r');
  try {
    const buffer = Buffer.allocUnsafe(64 * 1024);
    let offset = 0;
    for (;;) {
      const count = readSync(handle, buffer, 0, buffer.length, offset);
      if (count === 0) break;
      offset += count;
      await new Promise((done, reject) => {
        destination.write(buffer.subarray(0, count), (error) => {
          if (error) reject(error);
          else done();
        });
      });
    }
  } finally {
    closeSync(handle);
  }
}

function cleanupChildLog(path, recordId, stream, recordDirectoryIdentity) {
  const inspected = spawnSync(
    deleter,
    ['--inspect-cleanup-log', path, recordId, stream, recordDirectoryIdentity],
    {
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (![0, 3].includes(inspected.status)) {
    throw new Error('native namespace child log authentication failed');
  }
  const log = inspected.status === 0 ? JSON.parse(inspected.stdout) : null;
  const deleted = spawnSync(
    deleter,
    [
      '--delete-cleanup-log',
      path,
      recordId,
      log?.sha256 ?? '-',
      log?.byteLength?.toString() ?? '-',
      log?.fileIdentity ?? '-',
      stream,
      recordDirectoryIdentity,
    ],
    {
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  if (deleted.status === 197) {
    throw new SupervisorFailure(197, `injected ${stream} log deletion crash`);
  }
  if (deleted.status !== 0) throw new Error('native namespace child log deletion failed');
}

async function nextNamespaceSessionEvent(iterator, nonce) {
  const prefix = `TQNS:${nonce}:`;
  for (;;) {
    const line = await iterator.next();
    if (line.done) return null;
    if (!line.value.startsWith(prefix)) {
      throw new Error('native namespace control channel returned unauthenticated data');
    }
    return JSON.parse(line.value.slice(prefix.length));
  }
}

function assertSupervisorExited(record) {
  if (record.supervisor && processIdentityAlive(record.supervisor)) {
    throw new Error('native namespace supervisor still has its recorded process identity');
  }
}

function handleNamespaceSessionExit(result, record) {
  assertSupervisorExited(record);
  if (result.signal) throw new Error(`native namespace supervisor ended with ${result.signal}`);
  throw new SupervisorFailure(
    result.code,
    `native namespace supervisor failed during setup: ${result.code}`,
  );
}

function ensureProtectedRecordRoot() {
  for (const path of [resolve(root, 'tmp'), stateRoot, recordRoot]) {
    if (!existsSync(path)) {
      mkdirSync(path);
      fsyncDirectory(path);
      fsyncDirectory(dirname(path));
    }
  }
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
  ensureProtectedRecordRoot();
  const path = recordPath(record.recordId);
  const previousBytes = existsSync(path) ? readFileSync(path) : null;
  const previous = previousBytes === null ? null : readRecord(path);
  record.revision = (previous?.revision ?? -1) + 1;
  validateRecord(record);
  const previousSha256 =
    previousBytes === null ? '-' : createHash('sha256').update(previousBytes).digest('hex');
  const published = spawnSync(deleter, ['--publish-record', path, previousSha256], {
    input: `${JSON.stringify(record)}\n`,
    encoding: 'utf8',
    windowsHide: true,
    env: process.env,
  });
  if (published.status === 197) {
    throw new SupervisorFailure(197, 'injected native cleanup-record publication crash');
  }
  if (published.status !== 0) {
    throw new Error(`native cleanup-record publication failed: ${published.stderr.trim()}`);
  }
  Object.assign(record, readRecord(path));
  return record;
}

function effectiveLegacyEvidencePhase(record) {
  return record.phase === 'logs-preserved' ? record.logsPreservedFromPhase : record.phase;
}

function migrateCleanupRecord(record) {
  if (record.schemaVersion === 4) {
    if (record.logsPreserved === true) {
      const authenticated = authenticateLegacyEvidenceRecord(record, false);
      record.legacyEvidenceMigration = {
        rootIdentity: authenticated.identity,
        effectivePhase: effectiveLegacyEvidencePhase(record),
        artifacts: authenticated.artifacts,
      };
    }
    record.schemaVersion = RECORD_SCHEMA;
    record.logsRetired ??= false;
    writeRecord(record);
    return record;
  }
  if (record.schemaVersion !== NATIVE_SESSION_SCHEMA) return record;
  // 73f4391 schema-3 records predate control metadata; eae25e3 records contain both fields.
  const hasControlNonce = record.controlNonce !== undefined;
  const hasChildLog = record.childLogFile !== undefined;
  if (hasControlNonce !== hasChildLog) {
    throw new Error('schema-3 cleanup record control fields are partial');
  }
  if (
    hasControlNonce &&
    (!/^[0-9a-f]{32}$/u.test(record.controlNonce) ||
      record.childLogFile !== `${record.recordId}.log`)
  ) {
    throw new Error('schema-3 cleanup record control fields are invalid');
  }
  if (record.childLogFile !== undefined) {
    record.legacyChildLogFile = record.childLogFile;
  }
  delete record.childLogFile;
  record.schemaVersion = RECORD_SCHEMA;
  record.controlNonce ??= randomBytes(16).toString('hex');
  record.childStdoutLogFile = `${record.recordId}.stdout.log`;
  record.childStderrLogFile = `${record.recordId}.stderr.log`;
  record.logsRetired ??= false;
  writeRecord(record);
  return record;
}

function recoverRecordedNamespaces(deleter, ownerMayBeCurrent = false) {
  inspectLegacyEvidenceRoot();
  if (!existsSync(recordRoot)) {
    retireOrphanLegacyEvidence();
    return;
  }
  if (!isPlainPath(recordRoot, true)) throw new Error('cleanup record root is not plain');
  recoverNativeRecordBackups();
  recoverNativeRecordTemps();
  const files = readdirSync(recordRoot).sort();
  for (const name of files) {
    if (
      !/^[0-9a-f]{32}\.(?:json|log|stdout\.log|stderr\.log)$/u.test(name) &&
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
  const expectedLogs = new Set(
    records.flatMap((record) => {
      if (
        [4, RECORD_SCHEMA].includes(record.schemaVersion) &&
        record.logsRetired !== true &&
        record.logsPreserved !== true
      ) {
        return [
          record.childStdoutLogFile,
          record.childStderrLogFile,
          ...(record.legacyChildLogFile === undefined ? [] : [record.legacyChildLogFile]),
        ];
      }
      if (record.schemaVersion === NATIVE_SESSION_SCHEMA && record.childLogFile !== undefined) {
        return [record.childLogFile];
      }
      return [];
    }),
  );
  for (const name of files.filter((entry) => entry.endsWith('.log'))) {
    if (!expectedLogs.has(name)) throw new Error(`orphan machine-lock child log: ${name}`);
  }
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
  for (let value of records) {
    if (
      (processIdentityAlive(value.owner) &&
        !(ownerMayBeCurrent && value.owner.Pid === process.pid)) ||
      processIdentityAlive(value.child) ||
      processIdentityAlive(value.supervisor)
    ) {
      throw new Error(`machine-lock cleanup record is owned by a live process: ${value.recordId}`);
    }
    cleanupRecord(value, deleter, ownerMayBeCurrent);
  }
  retireOrphanLegacyEvidence();
  removeEmptyParents();
}

function recoverNativeRecordBackups() {
  if (!existsSync(recordRoot)) return;
  for (const name of readdirSync(recordRoot).sort()) {
    if (!/^[0-9a-f]{32}\.previous-v1$/u.test(name)) continue;
    const recovered = spawnSync(deleter, ['--recover-record-backup', resolve(recordRoot, name)], {
      encoding: 'utf8',
      windowsHide: true,
    });
    if (recovered.status !== 0) {
      throw new Error(`cannot recover native cleanup-record previous file ${name}`);
    }
  }
}

function recoverNativeRecordTemps() {
  if (!existsSync(recordRoot)) return;
  const pending = readdirSync(recordRoot)
    .filter((name) => /^[0-9a-f]{32}\.native-[0-9a-f]{32}\.pending-v1$/u.test(name))
    .sort();
  const ids = pending.map((name) => name.slice(0, 32));
  if (new Set(ids).size !== ids.length) {
    throw new Error('multiple native cleanup-record temporaries target one record');
  }
  for (const name of pending) {
    const path = resolve(recordRoot, name);
    const inspected = spawnSync(deleter, ['--inspect-record-temp', path], {
      windowsHide: true,
    });
    if (inspected.status !== 0) {
      throw new Error(`cannot inspect native cleanup-record temporary ${name}`);
    }
    try {
      const candidate = JSON.parse(inspected.stdout.toString('utf8'));
      validateRecord(candidate);
      if (candidate.recordId !== name.slice(0, 32)) {
        throw new Error('native cleanup-record temporary identity changed');
      }
    } catch (error) {
      if (!(error instanceof SyntaxError)) throw error;
    }
    const candidateSha256 = createHash('sha256').update(inspected.stdout).digest('hex');
    const recovered = spawnSync(deleter, ['--recover-record-temp', path, candidateSha256], {
      encoding: 'utf8',
      windowsHide: true,
    });
    if (recovered.status !== 0) {
      throw new Error(
        `cannot recover native cleanup-record temporary ${name}: ${recovered.stderr}`,
      );
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
  if (record.logsPreserved === true || record.legacyEvidenceMigration !== undefined) {
    authenticateLegacyEvidenceNamespaceRoots(record);
  }
  record = migrateCleanupRecord(record);
  if (record.legacyEvidenceMigration !== undefined) retireLegacyEvidence(record);
  else retireRecoveredLogs(record);
  if (record.roots.length !== rootKinds.length || record.supervisor === null) {
    record.partialCreation = true;
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
      if (
        rootRecord.identity === null &&
        (record.creatingRoot === rootRecord.kind ||
          (record.schemaVersion >= NATIVE_SESSION_SCHEMA && record.phase === 'native-creating'))
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
        (record.schemaVersion >= NATIVE_SESSION_SCHEMA && record.phase === 'native-creating'))
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
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'inventory-sealed') {
    throw new SupervisorFailure(197, 'injected inventory-sealed recovery crash');
  }

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
      throw new SupervisorFailure(197, 'injected root-deletion recovery crash');
    }
  }
  record.phase = 'filesystem-deleted';
  writeRecord(record);
  record.phase = 'deleting-registry';
  writeRecord(record);
  const registryNow = registrySnapshot(record.namespaceId);
  if (registryNow.present) {
    const expected =
      record.schemaVersion >= NATIVE_SESSION_SCHEMA && record.phase === 'deleting-registry'
        ? registryNow
        : registryInventory;
    if (
      record.schemaVersion >= NATIVE_SESSION_SCHEMA &&
      record.phase === 'deleting-registry' &&
      !registryInventorySubset(registryNow, registryInventory)
    ) {
      throw new Error('partially deleted registry namespace is not an exact recorded subset');
    }
    deleteExactRegistry(record.namespaceId, expected);
  }
  if (process.env.TQ_MACHINE_LOCK_TEST_CRASH_AFTER === 'registry-deleted') {
    throw new SupervisorFailure(197, 'injected registry-deletion recovery crash');
  }
  record.phase = 'registry-deleted';
  writeRecord(record);
  deleteCleanupRecord(record.recordId);
  removeEmptyParents();
}

function assertRetiredRecordNamespaceIsGone(record) {
  for (const rootRecord of record.roots) {
    if (
      existsSync(namespaceRoot(rootRecord.kind, record.namespaceId)) ||
      existsSync(resolve(recordRoot, rootRecord.bindingFile)) ||
      existsSync(`${resolve(recordRoot, rootRecord.bindingFile)}.intent-v1`)
    ) {
      throw new Error('cleanup record retired before its filesystem namespace');
    }
  }
  if (registrySnapshot(record.namespaceId).present) {
    throw new Error('cleanup record retired before its registry namespace');
  }
}

function deleteCleanupRecord(recordId) {
  const path = recordPath(recordId);
  if (!isPlainPath(path, false)) throw new Error('cleanup record changed before deletion');
  const deleted = spawnSync(deleter, ['--delete-cleanup-record', path], {
    encoding: 'utf8',
    windowsHide: true,
    env: process.env,
  });
  if (deleted.status === 197) {
    throw new SupervisorFailure(197, 'injected cleanup-record retirement crash');
  }
  if (deleted.status !== 0) {
    throw new Error(`native cleanup-record retirement failed: ${deleted.stderr.trim()}`);
  }
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

function authenticateLegacyEvidenceNamespaceRoots(record) {
  const phase = effectiveLegacyEvidencePhase(record);
  for (const rootRecord of record.roots) {
    const path = namespaceRoot(rootRecord.kind, record.namespaceId);
    if (record.deletedRoots.includes(rootRecord.kind)) {
      const binding = resolve(recordRoot, rootRecord.bindingFile);
      if (existsSync(path) || existsSync(binding) || existsSync(`${binding}.intent-v1`)) {
        throw new Error(`retired legacy evidence namespace root has residue: ${rootRecord.kind}`);
      }
      continue;
    }
    if (!existsSync(path)) {
      if (record.deletingRoot === rootRecord.kind) {
        verifyDeletedRootBinding(rootRecord);
        continue;
      }
      throw new Error(`legacy evidence namespace root is missing: ${rootRecord.kind}`);
    }
    if (rootRecord.identity === null) {
      throw new Error(`legacy evidence namespace root identity is absent: ${rootRecord.kind}`);
    }
    verifyRootBinding(path, rootRecord);
    if (ownedTreeIdentity(path) !== rootRecord.identity) {
      throw new Error(`legacy evidence namespace root identity changed: ${rootRecord.kind}`);
    }
    const inventory = inspectTree(path);
    if (
      [
        'inventory-sealed',
        'deleting-root',
        'filesystem-deleted',
        'deleting-registry',
        'registry-deleted',
      ].includes(phase) &&
      JSON.stringify(inventory) !== JSON.stringify(rootRecord.inventory) &&
      !(
        record.deletingRoot === rootRecord.kind &&
        exactInventorySubset(inventory, rootRecord.inventory)
      )
    ) {
      throw new Error(`legacy evidence namespace root inventory changed: ${rootRecord.kind}`);
    }
  }
}

function verifyDeletedRootBinding(rootRecord) {
  const result = spawnSync(
    deleter,
    [
      '--verify-deleted-root-binding',
      resolve(recordRoot, rootRecord.bindingFile),
      rootRecord.ownershipPrefix,
      rootRecord.bindingNonce,
      rootRecord.identity,
      rootRecord.adsSha256,
    ],
    { encoding: 'utf8', windowsHide: true },
  );
  if (result.status !== 0) {
    throw new Error(`deleted protected root binding changed: ${rootRecord.kind}`);
  }
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
  if (result.status === 197) {
    throw new SupervisorFailure(197, 'injected cleanup-binding recovery crash');
  }
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
  if (
    (record.schemaVersion >= 4 && record.revision === undefined) ||
    (record.revision !== undefined &&
      (!Number.isSafeInteger(record.revision) || record.revision < 0))
  ) {
    throw new Error('cleanup record revision is invalid');
  }
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
  if (record.schemaVersion === NATIVE_SESSION_SCHEMA) {
    const hasControlNonce = record.controlNonce !== undefined;
    const hasChildLog = record.childLogFile !== undefined;
    if (
      hasControlNonce !== hasChildLog ||
      (hasControlNonce &&
        (!/^[0-9a-f]{32}$/u.test(record.controlNonce) ||
          record.childLogFile !== `${record.recordId}.log`)) ||
      record.childStdoutLogFile !== undefined ||
      record.childStderrLogFile !== undefined
    ) {
      throw new Error('schema-3 cleanup record control identity is invalid');
    }
  }
  if (
    record.schemaVersion >= 4 &&
    (!/^[0-9a-f]{32}$/u.test(record.controlNonce ?? '') ||
      record.childStdoutLogFile !== `${record.recordId}.stdout.log` ||
      record.childStderrLogFile !== `${record.recordId}.stderr.log` ||
      (record.legacyChildLogFile !== undefined &&
        record.legacyChildLogFile !== `${record.recordId}.log`))
  ) {
    throw new Error('cleanup record control identity is invalid');
  }
  if (
    record.schemaVersion >= NATIVE_SESSION_SCHEMA &&
    record.phase !== 'native-creating' &&
    record.partialCreation !== true &&
    !validProcessIdentity(record.supervisor)
  ) {
    throw new Error('cleanup record supervisor identity is invalid');
  }
  if (record.logsRetired !== undefined && typeof record.logsRetired !== 'boolean') {
    throw new Error('cleanup record log-retirement state is invalid');
  }
  const hasLegacyEvidence =
    (record.schemaVersion === 4 && record.logsPreserved === true) ||
    (record.schemaVersion === RECORD_SCHEMA && record.legacyEvidenceMigration !== undefined);
  if (hasLegacyEvidence) {
    if (
      ![
        'logs-preserved',
        'inventory-sealed',
        'deleting-root',
        'filesystem-deleted',
        'deleting-registry',
        'registry-deleted',
      ].includes(record.phase) ||
      record.logsPreserved !== true ||
      record.logsRetired === true ||
      ![
        'prepared',
        'native-creating',
        'roots-created',
        'inventory-sealed',
        'deleting-root',
        'filesystem-deleted',
        'deleting-registry',
        'registry-deleted',
      ].includes(record.logsPreservedFromPhase) ||
      !Array.isArray(record.recoveredLogEvidence) ||
      !/^\d+:\d+$/u.test(record.evidenceDirectoryIdentity ?? '') ||
      typeof record.evidenceDirectoryAcl !== 'string'
    ) {
      throw new Error('legacy recovered-log evidence record is invalid');
    }
    const channels = recoveredLogPlans(record).map((entry) => entry.stream);
    if (
      record.recoveredLogEvidence.length !== channels.length ||
      record.recoveredLogEvidence.some((entry, index) => entry.channel !== channels[index])
    ) {
      throw new Error('legacy recovered-log evidence channels are invalid');
    }
    for (const entry of record.recoveredLogEvidence) {
      if (
        entry.fileName !== `${record.recordId}.${entry.channel}.evidence-v1` ||
        typeof entry.present !== 'boolean' ||
        (entry.present &&
          (!Number.isSafeInteger(entry.byteLength) ||
            entry.byteLength < 0 ||
            !/^[0-9a-f]{64}$/u.test(entry.sha256 ?? ''))) ||
        (!entry.present && (entry.byteLength !== undefined || entry.sha256 !== undefined))
      ) {
        throw new Error('legacy recovered-log evidence metadata is invalid');
      }
    }
    if (record.schemaVersion === RECORD_SCHEMA) {
      const migration = record.legacyEvidenceMigration;
      const present = record.recoveredLogEvidence.filter((entry) => entry.present);
      if (
        migration?.rootIdentity !== record.evidenceDirectoryIdentity ||
        migration?.effectivePhase !== effectiveLegacyEvidencePhase(record) ||
        !Array.isArray(migration?.artifacts) ||
        migration.artifacts.length !== present.length
      ) {
        throw new Error('legacy evidence migration checkpoint is invalid');
      }
      for (let index = 0; index < present.length; index += 1) {
        const artifact = migration.artifacts[index];
        const expected = present[index];
        if (
          artifact.channel !== expected.channel ||
          artifact.fileName !== expected.fileName ||
          artifact.byteLength !== expected.byteLength ||
          artifact.sha256 !== expected.sha256 ||
          !/^[0-9a-f]{32}$/u.test(artifact.fileIdentity ?? '') ||
          !['pending', 'retiring', 'retired'].includes(artifact.status)
        ) {
          throw new Error('legacy evidence migration artifact is invalid');
        }
      }
    }
  } else if (
    record.logsPreserved !== undefined ||
    record.logsPreservedFromPhase !== undefined ||
    record.recoveredLogEvidence !== undefined ||
    record.evidenceDirectoryIdentity !== undefined ||
    record.evidenceDirectoryAcl !== undefined ||
    record.legacyEvidenceMigration !== undefined ||
    record.phase === 'logs-preserved'
  ) {
    throw new Error('unexpected legacy recovered-log evidence metadata');
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
      (record.schemaVersion >= NATIVE_SESSION_SCHEMA &&
        !/^\d+:\d+$/u.test(entry.parentIdentity ?? '')) ||
      (entry.parentIdentity !== undefined && !/^\d+:\d+$/u.test(entry.parentIdentity)) ||
      !/^[0-9a-f]{32}$/u.test(entry.bindingNonce ?? '') ||
      entry.bindingFile !== `${record.recordId}.${entry.kind}.binding-v1` ||
      (entry.adsSha256 !== null && !/^[0-9a-f]{64}$/u.test(entry.adsSha256)) ||
      (entry.identity === null && entry.adsSha256 !== null) ||
      (entry.identity !== null && entry.adsSha256 === null) ||
      (entry.identity === null &&
        record.schemaVersion < NATIVE_SESSION_SCHEMA &&
        record.creatingRoot !== entry.kind &&
        !record.deletedRoots.includes(entry.kind)) ||
      (entry.identity === null &&
        record.schemaVersion >= NATIVE_SESSION_SCHEMA &&
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

function reloadProtectedRecord(recordId) {
  const path = recordPath(recordId);
  assertProtectedRecord(path);
  const latest = JSON.parse(readFileSync(path, 'utf8'));
  validateRecord(latest);
  if (latest.recordId !== recordId) {
    throw new Error('native namespace cleanup record identity changed');
  }
  return latest;
}

function recordPath(recordId) {
  assertNamespaceId(recordId);
  return resolve(recordRoot, `${recordId}.json`);
}

function removeEmptyParents() {
  if (existsSync(recordRoot) && readdirSync(recordRoot).length === 0) {
    rmdirSync(recordRoot);
    fsyncDirectory(stateRoot);
  }
  for (const kind of rootKinds) {
    const path = resolve(stateRoot, kind);
    if (existsSync(path) && readdirSync(path).length === 0) {
      rmdirSync(path);
      fsyncDirectory(stateRoot);
    }
  }
  removeEmptyRegistryRoot();
  if (existsSync(stateRoot) && readdirSync(stateRoot).length === 0) {
    rmdirSync(stateRoot);
    fsyncDirectory(resolve(root, 'tmp'));
  }
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
