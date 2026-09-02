import { spawn, spawnSync } from 'node:child_process';
import { createHash, randomBytes } from 'node:crypto';
import {
  existsSync,
  linkSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
  renameSync,
  rmdirSync,
  symlinkSync,
  unlinkSync,
  writeFileSync,
} from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const run = process.platform === 'win32' ? describe.sequential : describe.skip;
const wrapper = resolve('scripts', 'run-machine-lock-isolated-tests.mjs');
const records = resolve('tmp', 'machine-lock-tests', '.cleanup-records-v1');
const nativeHelper = resolve(
  'tmp',
  'cargo-target',
  'machine-lock-test-wrapper',
  'debug',
  'talking-quill-test-tree-delete.exe',
);

run('Windows machine-lock wrapper teardown', () => {
  it.each([
    'native-record-temp-created',
    'native-record-temp-parent-flushed',
    'native-record-temp-partial-written',
    'native-record-temp-written',
    'native-record-temp-file-flushed',
    'native-record-temp-renamed',
    'native-record-destination-parent-flushed',
    'cleanup-record-retired',
    'cleanup-record-retirement-directory-flushed',
    'intent-partial-write',
    'intent-written-before-flush',
    'intent-file-flushed',
    'intent-parent-flushed',
    'intent-flushed-before-root',
    'create-before-binding',
    'binding-partial-write',
    'binding-written-before-flush',
    'binding-file-flushed',
    'binding-parent-flushed',
    'binding-flushed-before-ads',
    'ads-partial-write',
    'ads-written-before-flush',
    'ads-file-flushed',
    'ads-parent-flushed',
    'ads-flushed-before-return',
    'create-after-record-before-identity',
    'roots-created',
    'child-stdout-log-created',
    'child-stderr-log-created',
    'child-log-directory-flushed',
    'child-stdout-log-flushed',
    'child-stderr-log-flushed',
    'inventory-sealed',
    'deleted-root:helper',
    'binding-deleted-before-intent',
    'registry-values-deleted',
    'registry-recovery-deleted',
    'registry-namespace-values-deleted',
    'registry-namespace-deleted',
    'registry-deleted',
  ])(
    'recovers a durable record after a crash at %s',
    (phase) => {
      const command = phase.startsWith('registry-')
        ? 'node tests/fixtures/machine-lock-test-registry-state.mjs'
        : 'cmd.exe /d /c exit 0';
      const crashed = runWrapper(command, {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      });
      expect(crashed.status, crashed.stderr).toBe(197);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

  it.each([
    'recovered-log-authenticated:stdout',
    'recovered-log-emitted:stdout',
    'recovered-log-retired:stdout',
    'recovered-log-retirement-directory-flushed:stdout',
    'recovered-log-authenticated:stderr',
    'recovered-log-emitted:stderr',
    'recovered-log-retired:stderr',
    'recovered-log-retirement-directory-flushed:stderr',
    'recovered-logs-retired',
    'logs-retired-record-published',
  ])(
    'retires recovered logs after a crash at %s',
    async (phase) => {
      await leaveRecordAtPhase(
        'inventory-sealed',
        'cmd.exe /d /c echo durable-stdout ^& echo durable-stderr 1^>^&2',
      );
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      });
      expect(crashed.status, crashed.stderr).toBe(197);
      expect(existsSync(records)).toBe(false);
      for (const frame of recoveredLogFrames(crashed.stderr)) assertRecoveredLogFrame(frame);
      expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
    },
    120_000,
  );

  it.each([
    'native-record-temp-created',
    'native-record-temp-parent-flushed',
    'native-record-temp-partial-written',
    'native-record-temp-written',
    'native-record-temp-file-flushed',
    'native-record-current-renamed',
    'native-record-current-rename-parent-flushed',
    'native-record-temp-renamed',
    'native-record-destination-parent-flushed',
    'native-record-previous-retired',
    'native-record-previous-retirement-parent-flushed',
  ])(
    'recovers a schema-3 migration publication crash at %s',
    async (phase) => {
      const record = await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c exit 0');
      cleanupFixtureLog(record.childStdoutLogFile);
      cleanupFixtureLog(record.childStderrLogFile);
      record.schemaVersion = 3;
      delete record.controlNonce;
      delete record.childStdoutLogFile;
      delete record.childStderrLogFile;
      writeFileSync(
        resolve(records, `${record.recordId}.json`),
        `${JSON.stringify(record)}\n`,
        'utf8',
      );
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
        TQ_MACHINE_LOCK_TEST_RECORD_CRASH_PHASE: 'inventory-sealed',
      });
      expect(crashed.status, crashed.stderr).toBe(197);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

  it('re-emits a deduplicable recovered-log frame after abrupt wrapper death', async () => {
    const record = await leaveRecordAtPhase(
      'inventory-sealed',
      'cmd.exe /d /c echo durable-wrapper-death-stdout ^& echo durable-wrapper-death-stderr 1^>^&2',
    );
    const token = randomBytes(16).toString('hex');
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-recovered-log`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    const child = spawn(process.execPath, [wrapper, '--', 'cmd.exe /d /c exit 0'], {
      env: {
        ...process.env,
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'recovered-log-emitted:stdout',
        TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
      },
      windowsHide: true,
      stdio: ['ignore', 'pipe', 'pipe'],
    });
    let stderr = '';
    child.stderr.on('data', (chunk) => {
      stderr += chunk.toString();
    });
    const completed = new Promise<number | null>((done, reject) => {
      child.once('error', reject);
      child.once('exit', done);
    });
    await waitForPath(`${pause}.ready`);
    const first = recoveredLogFrames(stderr);
    expect(first).toHaveLength(1);
    assertRecoveredLogFrame(first[0]);
    expect(first[0].recordId).toBe(record.recordId);
    expect(first[0].stream).toBe('stdout');
    expect(child.kill()).toBe(true);
    expect(await completed).not.toBe(0);
    unlinkSync(`${pause}.ready`);
    const retry = runWrapper('cmd.exe /d /c exit 0');
    expect(retry.status, retry.stderr).toBe(0);
    const repeated = recoveredLogFrames(retry.stderr);
    expect(repeated.some((frame) => frame.stream === 'stderr')).toBe(true);
    const repeatedStdout = repeated.find((frame) => frame.stream === 'stdout')!;
    expect(`${repeatedStdout.recordId}:${repeatedStdout.hash}`).toBe(
      `${first[0].recordId}:${first[0].hash}`,
    );
    expect(existsSync(records)).toBe(false);
    expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it('emits and retires a migrated schema-3 combined log', async () => {
    const record = await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c exit 0');
    const combinedPath = resolve(records, `${record.recordId}.log`);
    renameSync(resolve(records, record.childStdoutLogFile), combinedPath);
    cleanupFixtureLog(record.childStderrLogFile);
    record.schemaVersion = 3;
    record.childLogFile = `${record.recordId}.log`;
    delete record.childStdoutLogFile;
    delete record.childStderrLogFile;
    delete record.logsRetired;
    writeFileSync(combinedPath, 'legacy combined diagnostic\r\n', 'utf8');
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    const combined = recoveredLogFrames(recovered.stderr).find(
      (frame) => frame.stream === 'combined',
    );
    expect(combined).toBeDefined();
    assertRecoveredLogFrame(combined);
    expect(Buffer.from(combined.content, 'base64').toString('utf8')).toBe(
      'legacy combined diagnostic\r\n',
    );
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it('binds native log authority to the record, stream, and protected parent', async () => {
    const record = await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c echo bound-log');
    const rejected = spawnSync(
      nativeHelper,
      [
        '--inspect-cleanup-log',
        resolve(records, `${record.recordId}.json`),
        record.recordId,
        'stdout',
        record.recordDirectoryIdentity,
      ],
      { encoding: 'utf8' },
    );
    expect(rejected.status).not.toBe(0);
    expect(rejected.stderr).toContain('not record-bound');
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it('does not let a diagnostic write failure block cleanup', async () => {
    await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c echo best-effort-log');
    const recovered = runWrapper('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_DIAGNOSTIC_WRITE_FAIL: '1',
    });
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(recoveredLogFrames(recovered.stderr)).toEqual([]);
    expect(existsSync(records)).toBe(false);
    expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
  }, 120_000);

  it.each(['hardlink', 'reparse'] as const)(
    'rejects a post-seal %s and preserves its outside target',
    async (kind) => {
      const record = await leaveSealedRecord();
      const outside = resolve('tmp', 'machine-lock-wrapper-tests', `${record.namespaceId}-sealed`);
      const root = resolve('tmp', 'machine-lock-tests', 'helper', record.namespaceId);
      mkdirSync(outside, { recursive: true });
      const sentinel = resolve(outside, 'sentinel.txt');
      writeFileSync(sentinel, 'sealed sentinel\n', 'utf8');
      const attack = resolve(root, kind === 'hardlink' ? 'late-hardlink' : 'late-reparse');
      if (kind === 'hardlink') linkSync(sentinel, attack);
      else symlinkSync(outside, attack, 'junction');

      const rejected = runWrapper('cmd.exe /d /c exit 0');
      expect(rejected.status).not.toBe(0);
      expect(readFileSync(sentinel, 'utf8')).toBe('sealed sentinel\n');
      if (kind === 'hardlink') unlinkSync(attack);
      else rmdirSync(attack);

      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(readFileSync(sentinel, 'utf8')).toBe('sealed sentinel\n');
      unlinkSync(sentinel);
      rmdirSync(outside);
      const outsideParent = resolve('tmp', 'machine-lock-wrapper-tests');
      if (readdirSync(outsideParent).length === 0) rmdirSync(outsideParent);
    },
    120_000,
  );

  it('reports native stream inventory failures', () => {
    const result = spawnSync(nativeHelper, ['--stream-inventory', resolve('.')], {
      env: { ...process.env, TQ_MACHINE_LOCK_TEST_STREAM_INVENTORY_FAIL: '1' },
      encoding: 'utf8',
    });
    expect(result.status).not.toBe(0);
    expect(result.stderr).toContain('forced stream inventory failure');
  });

  it('inventories and hashes a directory ADS', () => {
    const directory = resolve('tmp', 'machine-lock-wrapper-tests', randomBytes(16).toString('hex'));
    mkdirSync(directory, { recursive: true });
    writeFileSync(`${directory}:probe`, 'directory stream\n', 'utf8');
    const result = spawnSync(nativeHelper, ['--stream-inventory', directory], { encoding: 'utf8' });
    expect(result.status, result.stderr).toBe(0);
    expect(JSON.parse(result.stdout)).toContainEqual({
      name: ':probe:$DATA',
      size: 17,
      sha256: '3c22970cd1b5bf1f4f24a19d2e412e48d9add19db886219e70bf725a649483a1',
    });
    unlinkSync(`${directory}:probe`);
    rmdirSync(directory);
    const parent = resolve(directory, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  });

  it('blocks every outer root rename and replacement during native publication', async () => {
    const token = randomBytes(16).toString('hex');
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-root-publication`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    const running = runWrapperAsync('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE: pause,
    });
    for (const kind of ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit']) {
      const seam = `${pause}.${kind}`;
      await waitForPath(`${seam}.ready`);
      const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
      const record = JSON.parse(readFileSync(resolve(records, recordName), 'utf8'));
      expect(record.creatingRoot).toBe(kind);
      const root = resolve('tmp', 'machine-lock-tests', kind, record.namespaceId);
      const moved = `${root}-moved`;
      const attacker = `${root}-replacement`;
      mkdirSync(attacker);
      expect(() => renameSync(root, moved)).toThrow();
      expect(() => rmdirSync(root)).toThrow();
      expect(
        spawnSync(nativeHelper, ['--force-directory-replacement', attacker, root]).status,
      ).toBe(0);
      expect(existsSync(root)).toBe(true);
      expect(existsSync(moved)).toBe(false);
      expect(existsSync(attacker)).toBe(true);
      rmdirSync(attacker);
      writeFileSync(`${seam}.continue`, 'continue\n', 'utf8');
    }
    const completed = await running;
    expect(completed.code, completed.stderr).toBe(0);
    for (const kind of ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit']) {
      unlinkSync(`${pause}.${kind}.ready`);
      unlinkSync(`${pause}.${kind}.continue`);
    }
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it('does not inherit the supervisor control handle into the child', () => {
    const result = runWrapper(
      `${nativeHelper} --assert-handle-not-inherited %TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE%`,
      { TQ_MACHINE_LOCK_TEST_PROBE_CONTROL_HANDLE: '1' },
    );
    expect(result.status, result.stderr).toBe(0);
  }, 120_000);

  it('protects supervisor process authority and isolates forged control frames', () => {
    const result = runWrapper('node tests/fixtures/machine-lock-test-control-forgery.mjs', {
      TQ_MACHINE_LOCK_TEST_CONTROL_HANDLE_VALUE: '123456',
      TQ_MACHINE_LOCK_TEST_SUPERVISOR_CLAIM:
        '{"pid":4,"creationTime":1,"imagePath":"spoof","imageIdentity":"1:1","imageSha256":"00"}',
    });
    expect(result.status, result.stderr).toBe(0);
    const forgedPrefix =
      'TQNS:00000000000000000000000000000000:{"event":"completed","childExitCode":0}';
    const stdoutLines = result.stdout.split(/\r?\n/u).filter((line) => line.startsWith('TQNS:'));
    const stderrLines = result.stderr
      .split(/\r?\n/u)
      .filter((line) => line.startsWith('child stderr'));
    expect(stdoutLines).toEqual(
      Array.from({ length: 256 }, () => `${forgedPrefix} ${'x'.repeat(1024)}`),
    );
    expect(stderrLines).toEqual(
      Array.from({ length: 256 }, (_, index) => `child stderr ${index} ${'y'.repeat(256)}`),
    );
    expect(result.stdout).not.toContain('child stderr');
    expect(result.stderr).not.toContain(forgedPrefix);
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it('returns a legitimate child exit code 197 only after completed teardown', () => {
    const result = runWrapper('cmd.exe /d /c exit 197');
    expect(result.status, result.stderr).toBe(197);
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it('fails closed when the latest native record changes before failure recovery', async () => {
    const token = randomBytes(16).toString('hex');
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-stale-record`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    const running = runWrapperAsync('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'inventory-sealed',
      TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
    });
    await waitForPath(`${pause}.ready`);
    const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
    const path = resolve(records, recordName);
    const latest = readFileSync(path, 'utf8');
    expect(JSON.parse(latest).phase).toBe('inventory-sealed');
    writeFileSync(path, '{}\n', 'utf8');
    writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
    expect((await running).code).not.toBe(197);
    expect(existsSync(path)).toBe(true);
    writeFileSync(path, latest, 'utf8');
    unlinkSync(`${pause}.ready`);
    unlinkSync(`${pause}.continue`);
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it.each(['hardlink', 'reparse'] as const)(
    'rejects a crashed native pending-record %s replacement',
    async (kind) => {
      const token = randomBytes(16).toString('hex');
      const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-native-pending`);
      mkdirSync(resolve(pause, '..'), { recursive: true });
      const running = runWrapperAsync('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'native-record-temp-file-flushed',
        TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
      });
      await waitForPath(`${pause}.ready`);
      const pendingName = readdirSync(records).find((name) =>
        /^[0-9a-f]{32}\.native-[0-9a-f]{32}\.pending-v1$/u.test(name),
      )!;
      const pending = resolve(records, pendingName);
      const outside = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-outside`);
      mkdirSync(outside);
      const sentinel = resolve(outside, 'sentinel');
      writeFileSync(sentinel, 'native pending sentinel\n', 'utf8');
      unlinkSync(pending);
      if (kind === 'hardlink') linkSync(sentinel, pending);
      else symlinkSync(sentinel, pending, 'file');
      writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
      expect((await running).code).not.toBe(197);
      expect(readFileSync(sentinel, 'utf8')).toBe('native pending sentinel\n');
      unlinkSync(pending);
      unlinkSync(sentinel);
      rmdirSync(outside);
      unlinkSync(`${pause}.ready`);
      unlinkSync(`${pause}.continue`);
      expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
      const parent = resolve(pause, '..');
      if (readdirSync(parent).length === 0) rmdirSync(parent);
    },
    120_000,
  );

  it('retains namespace root and parent handles while the child runs', () => {
    const result = runWrapper('node tests/fixtures/machine-lock-test-retained-parent.mjs');
    expect(result.status, result.stderr).toBe(0);
  }, 120_000);

  it.each([
    ['create-after-record-before-identity', 'cmd.exe /d /c exit 0'],
    ['roots-created', 'cmd.exe /d /c exit 0'],
    ['inventory-sealed', 'cmd.exe /d /c exit 0'],
    ['deleted-root:helper', 'cmd.exe /d /c exit 0'],
    ['registry-values-deleted', 'node tests/fixtures/machine-lock-test-registry-state.mjs'],
    ['registry-namespace-deleted', 'node tests/fixtures/machine-lock-test-registry-state.mjs'],
    ['registry-deleted', 'node tests/fixtures/machine-lock-test-registry-state.mjs'],
  ])(
    'migrates an authentic schema-3 native-session record at %s',
    async (phase, childCommand) => {
      const record = await leaveRecordAtPhase(phase, childCommand);
      cleanupFixtureLog(record.childStdoutLogFile);
      cleanupFixtureLog(record.childStderrLogFile);
      record.schemaVersion = 3;
      delete record.controlNonce;
      delete record.childStdoutLogFile;
      delete record.childStderrLogFile;
      writeFileSync(
        resolve(records, `${record.recordId}.json`),
        `${JSON.stringify(record)}\n`,
        'utf8',
      );
      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

  it('rejects partial schema-3 control metadata without cleanup', async () => {
    const record = await leaveSealedRecord();
    record.schemaVersion = 3;
    delete record.childStdoutLogFile;
    delete record.childStderrLogFile;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const rejected = runWrapper('cmd.exe /d /c exit 0');
    expect(rejected.status).not.toBe(0);
    expect(existsSync(resolve(records, `${record.recordId}.json`))).toBe(true);
    delete record.controlNonce;
    cleanupFixtureLog(`${record.recordId}.stdout.log`);
    cleanupFixtureLog(`${record.recordId}.stderr.log`);
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it('recovers a legacy cleanup record without parent identities', async () => {
    const record = await leaveSealedRecord();
    cleanupFixtureLog(record.childStdoutLogFile);
    cleanupFixtureLog(record.childStderrLogFile);
    record.schemaVersion = 1;
    for (const root of record.roots) delete root.parentIdentity;
    delete record.controlNonce;
    delete record.childStdoutLogFile;
    delete record.childStderrLogFile;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it('rejects an ownership ADS mutation after sealing', async () => {
    const record = await leaveSealedRecord();
    const rootRecord = record.roots.find((entry: { kind: string }) => entry.kind === 'helper');
    const root = resolve('tmp', 'machine-lock-tests', 'helper', record.namespaceId);
    const stream = `${root}:TalkingQuill.TestOwnership.V1`;
    writeFileSync(stream, 'mutated ownership\n', 'utf8');
    expect(runWrapper('cmd.exe /d /c exit 0').status).not.toBe(0);
    writeFileSync(stream, `${rootRecord.ownershipPrefix}:${rootRecord.identity}`, 'utf8');
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it('rejects a registry link type before namespace child creation', () => {
    const rootKey = 'HKCU\\Software\\Talking Quill Tests';
    const namespace = randomBytes(16).toString('hex');
    expect(spawnSync(nativeHelper, ['--registry-create-link-fixture']).status).toBe(0);
    expect(spawnSync(nativeHelper, ['--registry-create', namespace]).status).not.toBe(0);
    expect(spawnSync('reg.exe', ['query', `${rootKey}\\${namespace}`]).status).not.toBe(0);
    expect(spawnSync(nativeHelper, ['--registry-remove-link-fixture']).status).toBe(0);
  }, 120_000);

  it('preserves a registry child raced into empty-root deletion', async () => {
    const rootKey = 'HKCU\\Software\\Talking Quill Tests';
    const sibling = randomBytes(16).toString('hex');
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${sibling}-empty-registry`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    expect(spawnSync(nativeHelper, ['--registry-create-empty-root-fixture']).status).toBe(0);
    const child = spawn(nativeHelper, ['--registry-delete-empty-root'], {
      env: { ...process.env, TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE: pause },
      windowsHide: true,
    });
    const completed = new Promise<number | null>((done, reject) => {
      child.once('error', reject);
      child.once('exit', done);
    });
    await waitForPath(`${pause}.ready`);
    expect(spawnSync('reg.exe', ['add', `${rootKey}\\${sibling}`, '/f']).status).toBe(0);
    writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
    expect(await completed).not.toBe(0);
    expect(spawnSync('reg.exe', ['query', `${rootKey}\\${sibling}`]).status).toBe(0);
    expect(spawnSync('reg.exe', ['delete', `${rootKey}\\${sibling}`, '/f']).status).toBe(0);
    unlinkSync(`${pause}.ready`);
    unlinkSync(`${pause}.continue`);
    expect(spawnSync(nativeHelper, ['--registry-delete-empty-root']).status).toBe(0);
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it('rejects a registry value raced after native handle validation', async () => {
    const record = await leaveSealedRecord();
    const key = `HKCU\\Software\\Talking Quill Tests\\${record.namespaceId}`;
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${record.namespaceId}-registry`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    const running = runWrapperAsync('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE: pause,
    });
    await waitForPath(`${pause}.ready`);
    expect(
      spawnSync('reg.exe', ['add', key, '/v', 'late-race', '/t', 'REG_BINARY', '/d', '01', '/f'])
        .status,
    ).toBe(0);
    writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
    expect((await running).code).not.toBe(0);
    expect(spawnSync('reg.exe', ['query', key, '/v', 'late-race']).status).toBe(0);
    expect(spawnSync('reg.exe', ['delete', key, '/v', 'late-race', '/f']).status).toBe(0);
    unlinkSync(`${pause}.ready`);
    unlinkSync(`${pause}.continue`);
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it('rejects and preserves an unrecorded registry sibling', async () => {
    await leaveSealedRecord();
    const sibling = randomBytes(16).toString('hex');
    expect(spawnSync(nativeHelper, ['--registry-create', sibling]).status).toBe(0);
    expect(runWrapper('cmd.exe /d /c exit 0').status).not.toBe(0);
    const inventory = spawnSync(nativeHelper, ['--registry-inventory', sibling], {
      encoding: 'utf8',
    });
    expect(inventory.status).toBe(0);
    expect(
      spawnSync(nativeHelper, ['--registry-delete-exact', sibling], {
        input: inventory.stdout,
        encoding: 'utf8',
      }).status,
    ).toBe(0);
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it.each(['junction', 'symlink'] as const)(
    'rejects a malicious %s without traversing its target',
    (kind) => {
      const attempted = runWrapper(
        `node tests/fixtures/machine-lock-test-malicious-link.mjs ${kind}`,
      );
      if (attempted.status === 77) return;
      expect(attempted.status).not.toBe(0);

      const recordName = readdirSync(records).find((name) => name.endsWith('.json'));
      expect(recordName).toBeDefined();
      const record = JSON.parse(readFileSync(resolve(records, recordName!), 'utf8'));
      const outside = resolve('tmp', 'machine-lock-wrapper-tests', `${record.namespaceId}-outside`);
      const link = resolve(
        'tmp',
        'machine-lock-tests',
        'helper',
        record.namespaceId,
        kind === 'junction' ? 'malicious-junction' : 'malicious-symlink',
      );
      expect(lstatSync(link).isSymbolicLink()).toBe(true);
      expect(readFileSync(resolve(outside, 'sentinel.txt'), 'utf8')).toBe('outside sentinel\n');

      if (kind === 'junction') rmdirSync(link);
      else unlinkSync(link);
      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(readFileSync(resolve(outside, 'sentinel.txt'), 'utf8')).toBe('outside sentinel\n');
      unlinkSync(resolve(outside, 'sentinel.txt'));
      rmdirSync(outside);
      const outsideParent = resolve('tmp', 'machine-lock-wrapper-tests');
      if (readdirSync(outsideParent).length === 0) rmdirSync(outsideParent);
    },
    120_000,
  );
});

async function leaveSealedRecord() {
  return leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c exit 0');
}

async function leaveRecordAtPhase(phase: string, childCommand: string) {
  const token = randomBytes(16).toString('hex');
  const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-supervisor-failure`);
  mkdirSync(resolve(pause, '..'), { recursive: true });
  const child = spawn(process.execPath, [wrapper, '--', childCommand], {
    env: {
      ...process.env,
      TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
    },
    windowsHide: true,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  const completed = new Promise<number | null>((done, reject) => {
    child.once('error', reject);
    child.once('exit', done);
  });
  await waitForPath(`${pause}.ready`);
  const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
  const record = JSON.parse(readFileSync(resolve(records, recordName), 'utf8'));
  expect(child.kill()).toBe(true);
  expect(await completed).not.toBe(0);
  unlinkSync(`${pause}.ready`);
  const parent = resolve(pause, '..');
  if (readdirSync(parent).length === 0) rmdirSync(parent);
  return record;
}

async function waitForPath(path: string) {
  const deadline = Date.now() + 30_000;
  while (!existsSync(path)) {
    if (Date.now() >= deadline) throw new Error(`timed out waiting for ${path}`);
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

function runWrapperAsync(command: string, environment: NodeJS.ProcessEnv = {}) {
  const child = spawn(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    windowsHide: true,
  });
  let stderr = '';
  child.stderr?.on('data', (chunk) => {
    stderr += chunk.toString();
  });
  return new Promise<{ code: number | null; signal: NodeJS.Signals | null; stderr: string }>(
    (done, reject) => {
      child.once('error', reject);
      child.once('exit', (code, signal) => done({ code, signal, stderr }));
    },
  );
}

function cleanupFixtureLog(name: string) {
  const path = resolve(records, name);
  if (existsSync(path)) unlinkSync(path);
}

function recoveredLogFrames(stderr: string) {
  return stderr
    .split(/\r?\n/u)
    .filter((line) => line.startsWith('TQ_MACHINE_LOCK_RECOVERED_LOG:'))
    .map((line) => JSON.parse(line.slice('TQ_MACHINE_LOCK_RECOVERED_LOG:'.length)));
}

function assertRecoveredLogFrame(frame: {
  recordId: string;
  stream: string;
  hash: string;
  content: string;
}) {
  expect(Object.keys(frame).sort()).toEqual(['content', 'hash', 'recordId', 'stream']);
  expect(frame.recordId).toMatch(/^[0-9a-f]{32}$/u);
  expect(['stdout', 'stderr', 'combined']).toContain(frame.stream);
  const content = Buffer.from(frame.content, 'base64');
  expect(createHash('sha256').update(content).digest('hex')).toBe(frame.hash);
}

function runWrapper(command: string, environment: NodeJS.ProcessEnv = {}) {
  return spawnSync(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 110_000,
  });
}
