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
import { sanitizedSubprocessEnvironment } from '../../scripts/environment-policy.mjs';

const run = process.platform === 'win32' ? describe : describe.skip;
const wrapper = resolve('scripts', 'run-machine-lock-isolated-tests.mjs');
const records = resolve('tmp', 'machine-lock-tests', '.cleanup-records-v1');
// Hosted recovery can spend more than 30 seconds reaching a native pause. The
// native hook's own 30-second timer starts only after it publishes .ready.
const readinessTimeout = 75_000;
const wrapperTimeout = 110_000;
// Paused tests can run setup, the race, and recovery as separate wrappers.
const pausedTestTimeout = 3 * wrapperTimeout + 30_000;

type JsonObject = Record<string, unknown>;

interface InventoryEntry extends JsonObject {
  relativePath: string;
  directory: boolean;
  identity: string;
}

interface CleanupRoot extends JsonObject {
  kind: string;
  parentIdentity?: string;
  identity: string | null;
  inventory: InventoryEntry[];
  ownershipPrefix: string;
  bindingFile: string;
}

interface RecoveredLogEvidence extends JsonObject {
  channel: string;
  fileName: string;
  present: boolean;
  byteLength: number;
  sha256: string;
}

interface CleanupRecord extends JsonObject {
  schemaVersion: number;
  recordId: string;
  namespaceId: string;
  phase: string;
  recordDirectoryIdentity: string;
  creatingRoot: string | null;
  deletingRoot: string | null;
  deletedRoots: string[];
  roots: CleanupRoot[];
  controlNonce: string;
  childStdoutLogFile: string;
  childStderrLogFile: string;
  childLogFile?: string;
  logsRetired?: boolean;
  logsPreserved?: boolean;
  logsPreservedFromPhase?: string;
  recoveredLogEvidence?: RecoveredLogEvidence[];
  evidenceDirectoryIdentity?: string;
  evidenceDirectoryAcl?: string;
}

interface RecoveredLogFrame extends JsonObject {
  recordId: string;
  stream: string;
  hash: string;
  byteLength: number;
  contentPrefix: string;
  prefixByteLength: number;
  truncated: boolean;
  source: string;
  version: number;
}

interface StreamInventoryEntry extends JsonObject {
  name: string;
  size: number;
  sha256: string;
}

interface EvidenceRootInspection extends JsonObject {
  identity: string;
  names: string[];
}
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
      Reflect.deleteProperty(record, 'controlNonce');
      Reflect.deleteProperty(record, 'childStdoutLogFile');
      Reflect.deleteProperty(record, 'childStderrLogFile');
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

  it('migrates a schema-4 direct-log record to schema 5', async () => {
    const record = await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c echo schema-four');
    record.schemaVersion = 4;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(recoveredLogFrames(recovered.stderr).some((frame) => frame.stream === 'stdout')).toBe(
      true,
    );
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it.each(['logs-preserved', 'inventory-sealed', 'deleting-root'] as const)(
    'authenticates and migrates a schema-4 logs-preserved record at %s',
    async (phase) => {
      const record = await leaveSchema4LogsPreservedRecord(phase);
      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      const frames = recoveredLogFrames(recovered.stderr);
      expect(frames.map((frame) => frame.stream).sort()).toEqual(['stderr', 'stdout']);
      for (const frame of frames) assertRecoveredLogFrame(frame);
      expect(frames.every((frame) => frame.recordId === record.recordId)).toBe(true);
      expect(existsSync(records)).toBe(false);
      expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
    },
    120_000,
  );

  it('rejects changed schema-4 evidence before migration', async () => {
    const record = await leaveSchema4LogsPreservedRecord();
    const path = resolve(
      'tmp',
      'machine-lock-log-evidence-v1',
      `${record.recordId}.stdout.evidence-v1`,
    );
    const original = readFileSync(path);
    writeFileSync(path, 'changed legacy evidence\n', 'utf8');
    const rejected = runWrapper('cmd.exe /d /c exit 0');
    expect(rejected.status).not.toBe(0);
    expect(existsSync(resolve(records, `${record.recordId}.json`))).toBe(true);
    writeFileSync(path, original);
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(existsSync(records)).toBe(false);
    expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
  }, 120_000);

  it.each(['logs-preserved', 'deleting-root'] as const)(
    'authenticates %s namespace bindings before retiring schema-4 evidence',
    async (phase) => {
      const record = await leaveSchema4LogsPreservedRecord(phase);
      const bindingPath = resolve(
        records,
        findOrThrow(record.roots, () => true, 'missing cleanup root').bindingFile,
      );
      const original = readFileSync(bindingPath);
      writeFileSync(bindingPath, 'changed binding\n', 'utf8');
      const rejected = runWrapper('cmd.exe /d /c exit 0');
      expect(rejected.status).not.toBe(0);
      expect(
        existsSync(
          resolve('tmp', 'machine-lock-log-evidence-v1', `${record.recordId}.stdout.evidence-v1`),
        ),
      ).toBe(true);
      writeFileSync(bindingPath, original);
      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

  it('rejects binding residue for a root recorded as deleted before evidence migration', async () => {
    const record = await leaveSchema4LogsPreservedRecord('deleting-root');
    const kind = requiredString(record.deletingRoot, 'missing deleting root');
    record.deletedRoots.push(kind);
    record.deletingRoot = null;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const rejected = runWrapper('cmd.exe /d /c exit 0');
    expect(rejected.status).not.toBe(0);
    expect(
      existsSync(
        resolve('tmp', 'machine-lock-log-evidence-v1', `${record.recordId}.stdout.evidence-v1`),
      ),
    ).toBe(true);
    record.deletedRoots = record.deletedRoots.filter((value: string) => value !== kind);
    record.deletingRoot = kind;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(existsSync(records)).toBe(false);
  }, 120_000);

  it.each([
    'legacy-evidence-authenticated:stdout',
    'legacy-evidence-emitted:stdout',
    'legacy-evidence-retirement-intent:stdout',
    'legacy-evidence-retired:stdout',
    'legacy-evidence-directory-flushed:stdout',
    'legacy-evidence-retirement-recorded:stdout',
    'legacy-evidence-record-published',
    'legacy-evidence-root-retired',
  ])(
    'recovers a schema-4 evidence retirement crash at %s',
    async (phase) => {
      await leaveSchema4LogsPreservedRecord();
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      });
      expect(crashed.status, crashed.stderr).toBe(197);
      expect(existsSync(records)).toBe(false);
      expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
    },
    120_000,
  );

  it('recovers a schema-4 evidence migration publication crash', async () => {
    await leaveSchema4LogsPreservedRecord();
    const crashed = runWrapper('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'native-record-temp-renamed',
      TQ_MACHINE_LOCK_TEST_RECORD_CRASH_PHASE: 'logs-preserved',
    });
    expect(crashed.status, crashed.stderr).toBe(197);
    expect(existsSync(records)).toBe(false);
    expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
  }, 120_000);

  it.each(['final', 'pending'] as const)(
    'retires authenticated orphan %s evidence left after its record',
    (state) => {
      const recordId = randomBytes(16).toString('hex');
      const evidenceRoot = resolve('tmp', 'machine-lock-log-evidence-v1');
      mkdirSync(evidenceRoot, { recursive: true });
      const protectedRoot = spawnSync(
        nativeHelper,
        ['--protect-legacy-evidence-root', evidenceRoot],
        {
          encoding: 'utf8',
        },
      );
      expect(protectedRoot.status, protectedRoot.stderr).toBe(0);
      const path = resolve(
        evidenceRoot,
        `${recordId}.stdout.evidence-v1${state === 'pending' ? '.pending-v1' : ''}`,
      );
      const created = spawnSync(
        nativeHelper,
        ['--create-legacy-evidence-fixture', path, recordId, 'stdout', state],
        { input: 'orphan legacy diagnostic\n', encoding: 'utf8' },
      );
      expect(created.status, created.stderr).toBe(0);
      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      const frame = findOrThrow(
        recoveredLogFrames(recovered.stderr),
        () => true,
        'missing recovered log frame',
      );
      assertRecoveredLogFrame(frame);
      expect(frame.recordId).toBe(recordId);
      expect(existsSync(evidenceRoot)).toBe(false);
    },
    120_000,
  );

  it('hashes and retires a noisy child log larger than 128 MiB with bounded diagnostics', async () => {
    const record = await leaveRecordAtPhase(
      'inventory-sealed',
      'node tests/fixtures/machine-lock-test-noisy-child.mjs',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(recovered.stderr.length).toBeLessThan(200_000);
    const stdout = findOrThrow(
      recoveredLogFrames(recovered.stderr),
      (frame) => frame.stream === 'stdout',
      'missing recovered stdout frame',
    );
    assertRecoveredLogFrame(stdout);
    expect(stdout.recordId).toBe(record.recordId);
    expect(stdout.byteLength).toBe(129 * 1024 * 1024 + 1);
    expect(stdout.prefixByteLength).toBe(64 * 1024);
    expect(stdout.truncated).toBe(true);
    const expected = createHash('sha256');
    const chunk = Buffer.alloc(1024 * 1024, 0x61);
    for (let index = 0; index < 129; index += 1) expected.update(chunk);
    expected.update('z');
    expect(stdout.hash).toBe(expected.digest('hex'));
    expect(existsSync(records)).toBe(false);
  }, 240_000);

  it('retires an oversized noisy log when diagnostic emission fails', async () => {
    await leaveRecordAtPhase(
      'inventory-sealed',
      'node tests/fixtures/machine-lock-test-noisy-child.mjs',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_DIAGNOSTIC_WRITE_FAIL: '1',
    });
    expect(recovered.status, recovered.stderr).toBe(0);
    expect(recoveredLogFrames(recovered.stderr)).toEqual([]);
    expect(existsSync(records)).toBe(false);
    expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
  }, 240_000);

  it(
    're-emits a deduplicable recovered-log frame after abrupt wrapper death',
    async () => {
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
        timeout: wrapperTimeout,
        stdio: ['ignore', 'pipe', 'pipe'],
      });
      child.stdout.resume();
      let stderr = '';
      child.stderr.on('data', (chunk: unknown) => {
        stderr += streamChunkText(chunk);
      });
      const completed = new Promise<number | null>((done, reject) => {
        child.once('error', reject);
        child.once('close', done);
      });
      void completed.catch(() => undefined);
      try {
        await waitForPath(`${pause}.ready`);
        const frames = recoveredLogFrames(stderr);
        expect(frames).toHaveLength(1);
        const first = findOrThrow(frames, () => true, 'missing first recovered log frame');
        assertRecoveredLogFrame(first);
        expect(first.recordId).toBe(record.recordId);
        expect(first.stream).toBe('stdout');
        expect(child.kill()).toBe(true);
        expect(await completed).not.toBe(0);
        unlinkSync(`${pause}.ready`);
        const retry = runWrapper('cmd.exe /d /c exit 0');
        expect(retry.status, retry.stderr).toBe(0);
        const repeated = recoveredLogFrames(retry.stderr);
        expect(repeated.some((frame) => frame.stream === 'stderr')).toBe(true);
        const repeatedStdout = findOrThrow(
          repeated,
          (frame) => frame.stream === 'stdout',
          'missing repeated stdout frame',
        );
        expect(`${repeatedStdout.recordId}:${repeatedStdout.hash}`).toBe(
          `${first.recordId}:${first.hash}`,
        );
        expect(existsSync(records)).toBe(false);
        expect(existsSync(resolve('tmp', 'machine-lock-log-evidence-v1'))).toBe(false);
      } finally {
        if (child.exitCode === null && child.signalCode === null) child.kill();
        try {
          await completed;
        } finally {
          removePauseFiles(pause);
        }
      }
    },
    pausedTestTimeout,
  );

  it('emits and retires a migrated schema-3 combined log', async () => {
    const record = await leaveRecordAtPhase('inventory-sealed', 'cmd.exe /d /c exit 0');
    const combinedPath = resolve(records, `${record.recordId}.log`);
    renameSync(resolve(records, record.childStdoutLogFile), combinedPath);
    cleanupFixtureLog(record.childStderrLogFile);
    record.schemaVersion = 3;
    record.childLogFile = `${record.recordId}.log`;
    Reflect.deleteProperty(record, 'childStdoutLogFile');
    Reflect.deleteProperty(record, 'childStderrLogFile');
    delete record.logsRetired;
    writeFileSync(combinedPath, 'legacy combined diagnostic\r\n', 'utf8');
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const recovered = runWrapper('cmd.exe /d /c exit 0');
    expect(recovered.status, recovered.stderr).toBe(0);
    const combined = findOrThrow(
      recoveredLogFrames(recovered.stderr),
      (frame) => frame.stream === 'combined',
      'missing recovered combined frame',
    );
    assertRecoveredLogFrame(combined);
    expect(Buffer.from(combined.contentPrefix, 'base64').toString('utf8')).toBe(
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
    expect(parseStreamInventory(result.stdout)).toContainEqual({
      name: ':probe:$DATA',
      size: 17,
      sha256: '3c22970cd1b5bf1f4f24a19d2e412e48d9add19db886219e70bf725a649483a1',
    });
    unlinkSync(`${directory}:probe`);
    rmdirSync(directory);
    const parent = resolve(directory, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  });

  it(
    'blocks every outer root rename and replacement during native publication',
    async () => {
      const token = randomBytes(16).toString('hex');
      const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-root-publication`);
      mkdirSync(resolve(pause, '..'), { recursive: true });
      const running = runWrapperAsync('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE: pause,
      });
      const kinds = ['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit'];
      try {
        for (const kind of kinds) {
          const seam = `${pause}.${kind}`;
          await waitForPath(`${seam}.ready`, running);
          const recordName = findOrThrow(
            readdirSync(records),
            (name) => name.endsWith('.json'),
            'missing cleanup record',
          );
          const record = parseCleanupRecord(readFileSync(resolve(records, recordName), 'utf8'));
          expect(record.creatingRoot).toBe(kind);
          const root = resolve('tmp', 'machine-lock-tests', kind, record.namespaceId);
          const moved = `${root}-moved`;
          const attacker = `${root}-replacement`;
          mkdirSync(attacker);
          try {
            expect(() => renameSync(root, moved)).toThrow();
            expect(() => rmdirSync(root)).toThrow();
            expect(
              spawnSync(nativeHelper, ['--force-directory-replacement', attacker, root]).status,
            ).toBe(0);
            expect(existsSync(root)).toBe(true);
            expect(existsSync(moved)).toBe(false);
            expect(existsSync(attacker)).toBe(true);
          } finally {
            if (existsSync(attacker)) rmdirSync(attacker);
          }
          writeFileSync(`${seam}.continue`, 'continue\n', 'utf8');
        }
        const completed = await running;
        expect(completed.code, completed.stderr).toBe(0);
      } finally {
        // Release future roots too when an earlier root assertion fails.
        for (const kind of kinds) writeFileSync(`${pause}.${kind}.continue`, 'continue\n', 'utf8');
        try {
          await running;
        } finally {
          for (const kind of kinds) removePauseFiles(`${pause}.${kind}`);
        }
      }
    },
    pausedTestTimeout,
  );

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
      Array.from({ length: 256 }, (_, index) => `child stderr ${String(index)} ${'y'.repeat(256)}`),
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

  it(
    'fails closed when the latest native record changes before failure recovery',
    async () => {
      const token = randomBytes(16).toString('hex');
      const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-stale-record`);
      mkdirSync(resolve(pause, '..'), { recursive: true });
      const running = runWrapperAsync('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'inventory-sealed',
        TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
      });
      let original: { path: string; contents: string } | undefined;
      try {
        await waitForPath(`${pause}.ready`, running);
        const recordName = findOrThrow(
          readdirSync(records),
          (name) => name.endsWith('.json'),
          'missing cleanup record',
        );
        const path = resolve(records, recordName);
        const latest = readFileSync(path, 'utf8');
        original = { path, contents: latest };
        expect(parseCleanupRecord(latest).phase).toBe('inventory-sealed');
        writeFileSync(path, '{}\n', 'utf8');
        writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
        expect((await running).code).not.toBe(197);
        expect(existsSync(path)).toBe(true);
      } finally {
        await releasePause(pause, running);
        if (original) writeFileSync(original.path, original.contents, 'utf8');
        expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
      }
    },
    pausedTestTimeout,
  );

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
      const outside = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-outside`);
      const sentinel = resolve(outside, 'sentinel');
      let injectedPending: string | undefined;
      try {
        await waitForPath(`${pause}.ready`, running);
        const pendingName = findOrThrow(
          readdirSync(records),
          (name) => /^[0-9a-f]{32}\.native-[0-9a-f]{32}\.pending-v1$/u.test(name),
          'missing pending cleanup record',
        );
        const pending = resolve(records, pendingName);
        mkdirSync(outside);
        writeFileSync(sentinel, 'native pending sentinel\n', 'utf8');
        unlinkSync(pending);
        if (kind === 'hardlink') linkSync(sentinel, pending);
        else symlinkSync(sentinel, pending, 'file');
        injectedPending = pending;
        writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
        expect((await running).code).not.toBe(197);
        expect(readFileSync(sentinel, 'utf8')).toBe('native pending sentinel\n');
      } finally {
        await releasePause(pause, running);
        if (injectedPending) unlinkSync(injectedPending);
        if (existsSync(sentinel)) unlinkSync(sentinel);
        if (existsSync(outside)) rmdirSync(outside);
        expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
        removePauseFiles(pause);
      }
    },
    pausedTestTimeout,
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
      Reflect.deleteProperty(record, 'controlNonce');
      Reflect.deleteProperty(record, 'childStdoutLogFile');
      Reflect.deleteProperty(record, 'childStderrLogFile');
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
    Reflect.deleteProperty(record, 'childStdoutLogFile');
    Reflect.deleteProperty(record, 'childStderrLogFile');
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    const rejected = runWrapper('cmd.exe /d /c exit 0');
    expect(rejected.status).not.toBe(0);
    expect(existsSync(resolve(records, `${record.recordId}.json`))).toBe(true);
    Reflect.deleteProperty(record, 'controlNonce');
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
    Reflect.deleteProperty(record, 'controlNonce');
    Reflect.deleteProperty(record, 'childStdoutLogFile');
    Reflect.deleteProperty(record, 'childStderrLogFile');
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it('rejects an ownership ADS mutation after sealing', async () => {
    const record = await leaveSealedRecord();
    const rootRecord = findOrThrow(
      record.roots,
      (entry) => entry.kind === 'helper',
      'missing helper root',
    );
    const root = resolve('tmp', 'machine-lock-tests', 'helper', record.namespaceId);
    const stream = `${root}:TalkingQuill.TestOwnership.V1`;
    writeFileSync(stream, 'mutated ownership\n', 'utf8');
    expect(runWrapper('cmd.exe /d /c exit 0').status).not.toBe(0);
    writeFileSync(
      stream,
      `${rootRecord.ownershipPrefix}:${requiredString(rootRecord.identity, 'missing root identity')}`,
      'utf8',
    );
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

  it(
    'preserves a registry child raced into empty-root deletion',
    async () => {
      const rootKey = 'HKCU\\Software\\Talking Quill Tests';
      const sibling = randomBytes(16).toString('hex');
      const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${sibling}-empty-registry`);
      mkdirSync(resolve(pause, '..'), { recursive: true });
      expect(spawnSync(nativeHelper, ['--registry-create-empty-root-fixture']).status).toBe(0);
      const child = spawn(nativeHelper, ['--registry-delete-empty-root'], {
        env: { ...process.env, TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE: pause },
        windowsHide: true,
        timeout: wrapperTimeout,
      });
      child.stdout.resume();
      child.stderr.resume();
      const completed = new Promise<number | null>((done, reject) => {
        child.once('error', reject);
        child.once('close', done);
      });
      void completed.catch(() => undefined);
      try {
        await waitForPath(`${pause}.ready`);
        expect(spawnSync('reg.exe', ['add', `${rootKey}\\${sibling}`, '/f']).status).toBe(0);
        writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
        expect(await completed).not.toBe(0);
        expect(spawnSync('reg.exe', ['query', `${rootKey}\\${sibling}`]).status).toBe(0);
        expect(spawnSync('reg.exe', ['delete', `${rootKey}\\${sibling}`, '/f']).status).toBe(0);
      } finally {
        await releasePause(pause, completed);
        spawnSync('reg.exe', ['delete', `${rootKey}\\${sibling}`, '/f']);
        expect(spawnSync(nativeHelper, ['--registry-delete-empty-root']).status).toBe(0);
      }
    },
    pausedTestTimeout,
  );

  it(
    'rejects a registry value raced after native handle validation',
    async () => {
      const record = await leaveSealedRecord();
      const key = `HKCU\\Software\\Talking Quill Tests\\${record.namespaceId}`;
      const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${record.namespaceId}-registry`);
      mkdirSync(resolve(pause, '..'), { recursive: true });
      const running = runWrapperAsync('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_REGISTRY_DELETE_PAUSE_FILE: pause,
      });
      try {
        await waitForPath(`${pause}.ready`, running);
        expect(
          spawnSync('reg.exe', [
            'add',
            key,
            '/v',
            'late-race',
            '/t',
            'REG_BINARY',
            '/d',
            '01',
            '/f',
          ]).status,
        ).toBe(0);
        writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
        expect((await running).code).not.toBe(0);
        expect(spawnSync('reg.exe', ['query', key, '/v', 'late-race']).status).toBe(0);
        expect(spawnSync('reg.exe', ['delete', key, '/v', 'late-race', '/f']).status).toBe(0);
      } finally {
        await releasePause(pause, running);
        // Remove only the value this test injected, after the raced operation exits.
        spawnSync('reg.exe', ['delete', key, '/v', 'late-race', '/f']);
        expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
      }
    },
    pausedTestTimeout,
  );

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

      const recordName = findOrThrow(
        readdirSync(records),
        (name) => name.endsWith('.json'),
        'missing cleanup record',
      );
      const record = parseCleanupRecord(readFileSync(resolve(records, recordName), 'utf8'));
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
  const running = runWrapperAsync(childCommand, {
    TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
    TQ_MACHINE_LOCK_TEST_SUPERVISOR_FAILURE_PAUSE_FILE: pause,
  });
  try {
    await waitForPath(`${pause}.ready`, running);
    const recordName = findOrThrow(
      readdirSync(records),
      (name) => name.endsWith('.json'),
      'missing cleanup record',
    );
    const record = parseCleanupRecord(readFileSync(resolve(records, recordName), 'utf8'));
    expect(running.kill()).toBe(true);
    expect((await running).code).not.toBe(0);
    return record;
  } finally {
    // This fixture intentionally leaves a durable record, but never a live wrapper.
    if (!running.settled()) running.kill();
    try {
      await running;
    } finally {
      removePauseFiles(pause);
    }
  }
}

async function leaveSchema4LogsPreservedRecord(
  currentPhase: 'logs-preserved' | 'inventory-sealed' | 'deleting-root' = 'logs-preserved',
) {
  const record = await leaveRecordAtPhase(
    'inventory-sealed',
    'cmd.exe /d /c echo legacy-evidence-stdout ^& echo legacy-evidence-stderr 1^>^&2',
  );
  const evidenceRoot = resolve('tmp', 'machine-lock-log-evidence-v1');
  mkdirSync(evidenceRoot, { recursive: true });
  const protectedRoot = spawnSync(nativeHelper, ['--protect-legacy-evidence-root', evidenceRoot], {
    encoding: 'utf8',
  });
  expect(protectedRoot.status, protectedRoot.stderr).toBe(0);
  const evidence = [];
  for (const [channel, source] of [
    ['stdout', record.childStdoutLogFile],
    ['stderr', record.childStderrLogFile],
  ] as const) {
    const fileName = `${record.recordId}.${channel}.evidence-v1`;
    const destination = resolve(evidenceRoot, fileName);
    renameSync(resolve(records, source), destination);
    const bytes = readFileSync(destination);
    evidence.push({
      channel,
      fileName,
      present: true,
      byteLength: bytes.length,
      sha256: createHash('sha256').update(bytes).digest('hex'),
    });
  }
  const rootInspection = spawnSync(nativeHelper, ['--inspect-legacy-evidence-root', evidenceRoot], {
    encoding: 'utf8',
  });
  expect(rootInspection.status, rootInspection.stderr).toBe(0);
  const acl = spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      '(Get-Acl -LiteralPath $env:TQ_EVIDENCE_ROOT -ErrorAction Stop).Sddl',
    ],
    {
      env: sanitizedSubprocessEnvironment(process.env, { TQ_EVIDENCE_ROOT: evidenceRoot }),
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  expect(acl.status, acl.stderr).toBe(0);
  record.schemaVersion = 4;
  record.phase = currentPhase;
  if (currentPhase === 'deleting-root') {
    const root = findOrThrow(record.roots, () => true, 'missing cleanup root');
    const removed = spawnSync(
      nativeHelper,
      [
        '--exact',
        resolve('tmp', 'machine-lock-tests', root.kind, record.namespaceId),
        requiredString(root.identity, 'missing root identity'),
      ],
      { input: JSON.stringify(root.inventory), encoding: 'utf8' },
    );
    expect(removed.status, removed.stderr).toBe(0);
    record.deletingRoot = root.kind;
  }
  record.logsPreserved = true;
  record.logsPreservedFromPhase = 'inventory-sealed';
  record.recoveredLogEvidence = evidence;
  record.evidenceDirectoryIdentity = parseEvidenceRootInspection(rootInspection.stdout).identity;
  record.evidenceDirectoryAcl = acl.stdout.trim();
  delete record.logsRetired;
  writeFileSync(resolve(records, `${record.recordId}.json`), `${JSON.stringify(record)}\n`, 'utf8');
  return record;
}

async function waitForPath(path: string, running?: ReturnType<typeof runWrapperAsync>) {
  const started = Date.now();
  const deadline = started + readinessTimeout;
  while (!existsSync(path)) {
    if (running?.settled()) {
      const result = await running;
      throw new Error(`wrapper exited before ${path}: ${JSON.stringify(result)}`);
    }
    if (Date.now() >= deadline) {
      throw new Error(
        `timed out after ${String(Date.now() - started)}ms waiting for ${path}\n${running?.stderr() ?? ''}`,
      );
    }
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function releasePause(pause: string, completed: Promise<unknown>) {
  // Also release a hook that has not reached .ready yet. Await exit before
  // removing the signal, otherwise the late-arriving hook can remain blocked.
  writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
  try {
    // The test's readiness check or await reports spawn errors. Do not let the
    // same rejection prevent its remaining fixture cleanup in finally.
    await completed.catch(() => undefined);
  } finally {
    removePauseFiles(pause);
  }
}

function removePauseFiles(pause: string) {
  for (const suffix of ['.ready', '.continue']) {
    if (existsSync(`${pause}${suffix}`)) unlinkSync(`${pause}${suffix}`);
  }
  const parent = resolve(pause, '..');
  if (existsSync(parent) && readdirSync(parent).length === 0) rmdirSync(parent);
}

function runWrapperAsync(command: string, environment: NodeJS.ProcessEnv = {}) {
  const child = spawn(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    windowsHide: true,
    timeout: wrapperTimeout,
  });
  child.stdout.resume();
  let stderr = '';
  let settled = false;
  child.stderr.on('data', (chunk: unknown) => {
    stderr += streamChunkText(chunk);
  });
  const completed = new Promise<{
    code: number | null;
    signal: NodeJS.Signals | null;
    stderr: string;
  }>((done, reject) => {
    child.once('error', (error) => {
      settled = true;
      reject(error);
    });
    child.once('close', (code, signal) => {
      settled = true;
      done({ code, signal, stderr });
    });
  });
  // Readiness polling may still be pending when spawning fails.
  void completed.catch(() => undefined);
  return Object.assign(completed, {
    settled: () => settled,
    kill: () => child.kill(),
    stderr: () => stderr,
  });
}

function cleanupFixtureLog(name: string) {
  const path = resolve(records, name);
  if (existsSync(path)) unlinkSync(path);
}

function findOrThrow<T>(
  values: readonly T[],
  predicate: (value: T) => boolean,
  message: string,
): T {
  const value = values.find(predicate);
  if (value === undefined) throw new Error(message);
  return value;
}

function requiredString(value: string | null | undefined, message: string): string {
  if (value === null || value === undefined) throw new Error(message);
  return value;
}

function streamChunkText(chunk: unknown): string {
  if (typeof chunk === 'string') return chunk;
  if (Buffer.isBuffer(chunk)) return chunk.toString();
  throw new TypeError('child stream emitted a non-buffer chunk');
}

function parseJson(text: string): unknown {
  return JSON.parse(text) as unknown;
}

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function assertJsonObject(value: unknown, label: string): asserts value is JsonObject {
  if (!isJsonObject(value)) throw new TypeError(`${label} must be an object`);
}

function assertString(value: unknown, label: string): asserts value is string {
  if (typeof value !== 'string') throw new TypeError(`${label} must be a string`);
}

function assertNonnegativeInteger(value: unknown, label: string): asserts value is number {
  if (typeof value !== 'number' || !Number.isSafeInteger(value) || value < 0) {
    throw new TypeError(`${label} must be a nonnegative safe integer`);
  }
}

function assertBoolean(value: unknown, label: string): asserts value is boolean {
  if (typeof value !== 'boolean') throw new TypeError(`${label} must be a boolean`);
}

function assertNullableString(value: unknown, label: string): asserts value is string | null {
  if (value !== null) assertString(value, label);
}

function assertOptionalString(value: unknown, label: string): asserts value is string | undefined {
  if (value !== undefined) assertString(value, label);
}

function assertOptionalBoolean(
  value: unknown,
  label: string,
): asserts value is boolean | undefined {
  if (value !== undefined) assertBoolean(value, label);
}

function assertStringArray(value: unknown, label: string): asserts value is string[] {
  if (!Array.isArray(value) || !value.every((entry) => typeof entry === 'string')) {
    throw new TypeError(`${label} must be an array of strings`);
  }
}

function assertInventory(value: unknown, label: string): asserts value is InventoryEntry[] {
  if (!Array.isArray(value)) throw new TypeError(`${label} must be an array`);
  for (const [index, entry] of value.entries()) {
    assertJsonObject(entry, `${label} entry ${String(index)}`);
    assertString(entry.relativePath, `${label} entry ${String(index)} relative path`);
    assertBoolean(entry.directory, `${label} entry ${String(index)} directory flag`);
    assertString(entry.identity, `${label} entry ${String(index)} identity`);
  }
}

function assertCleanupRoot(value: unknown, index: number): asserts value is CleanupRoot {
  assertJsonObject(value, `cleanup root ${String(index)}`);
  assertString(value.kind, `cleanup root ${String(index)} kind`);
  if (!['helper', 'windows-setup', 'orphan-inventory', 'windows-setup-unit'].includes(value.kind)) {
    throw new TypeError(`cleanup root ${String(index)} kind is invalid`);
  }
  assertOptionalString(value.parentIdentity, `cleanup root ${String(index)} parent identity`);
  assertNullableString(value.identity, `cleanup root ${String(index)} identity`);
  assertInventory(value.inventory, `cleanup root ${String(index)} inventory`);
  assertString(value.ownershipPrefix, `cleanup root ${String(index)} ownership prefix`);
  assertString(value.bindingFile, `cleanup root ${String(index)} binding file`);
}

function assertRecoveredLogEvidence(
  value: unknown,
  index: number,
): asserts value is RecoveredLogEvidence {
  assertJsonObject(value, `recovered evidence ${String(index)}`);
  assertString(value.channel, `recovered evidence ${String(index)} channel`);
  assertString(value.fileName, `recovered evidence ${String(index)} file name`);
  assertBoolean(value.present, `recovered evidence ${String(index)} presence`);
  assertNonnegativeInteger(value.byteLength, `recovered evidence ${String(index)} byte length`);
  assertString(value.sha256, `recovered evidence ${String(index)} hash`);
  if (!/^[0-9a-f]{64}$/u.test(value.sha256)) {
    throw new TypeError(`recovered evidence ${String(index)} hash is invalid`);
  }
}

function assertCleanupRecord(value: unknown): asserts value is CleanupRecord {
  assertJsonObject(value, 'cleanup record');
  assertNonnegativeInteger(value.schemaVersion, 'cleanup record schema version');
  assertString(value.recordId, 'cleanup record id');
  assertString(value.namespaceId, 'cleanup record namespace id');
  if (!/^[0-9a-f]{32}$/u.test(value.recordId) || !/^[0-9a-f]{32}$/u.test(value.namespaceId)) {
    throw new TypeError('cleanup record identifiers are invalid');
  }
  assertString(value.phase, 'cleanup record phase');
  assertString(value.recordDirectoryIdentity, 'cleanup record directory identity');
  assertNullableString(value.creatingRoot, 'cleanup record creating root');
  assertNullableString(value.deletingRoot, 'cleanup record deleting root');
  assertStringArray(value.deletedRoots, 'cleanup record deleted roots');
  if (!Array.isArray(value.roots)) throw new TypeError('cleanup record roots must be an array');
  value.roots.forEach(assertCleanupRoot);
  assertString(value.controlNonce, 'cleanup record control nonce');
  assertString(value.childStdoutLogFile, 'cleanup record stdout log file');
  assertString(value.childStderrLogFile, 'cleanup record stderr log file');
  assertOptionalString(value.childLogFile, 'cleanup record combined log file');
  assertOptionalBoolean(value.logsRetired, 'cleanup record logs retired');
  assertOptionalBoolean(value.logsPreserved, 'cleanup record logs preserved');
  assertOptionalString(value.logsPreservedFromPhase, 'cleanup record preserved phase');
  assertOptionalString(value.evidenceDirectoryIdentity, 'cleanup record evidence identity');
  assertOptionalString(value.evidenceDirectoryAcl, 'cleanup record evidence ACL');
  if (value.recoveredLogEvidence !== undefined) {
    if (!Array.isArray(value.recoveredLogEvidence)) {
      throw new TypeError('cleanup record recovered evidence must be an array');
    }
    value.recoveredLogEvidence.forEach(assertRecoveredLogEvidence);
  }
}

function parseCleanupRecord(text: string): CleanupRecord {
  const value = parseJson(text);
  assertCleanupRecord(value);
  return value;
}

function assertRecoveredLogFrameValue(value: unknown): asserts value is RecoveredLogFrame {
  assertJsonObject(value, 'recovered log frame');
  assertString(value.recordId, 'recovered log record id');
  assertString(value.stream, 'recovered log stream');
  assertString(value.hash, 'recovered log hash');
  assertNonnegativeInteger(value.byteLength, 'recovered log byte length');
  assertString(value.contentPrefix, 'recovered log content prefix');
  assertNonnegativeInteger(value.prefixByteLength, 'recovered log prefix byte length');
  assertBoolean(value.truncated, 'recovered log truncation flag');
  assertString(value.source, 'recovered log source');
  assertNonnegativeInteger(value.version, 'recovered log version');
  if (
    !/^[0-9a-f]{32}$/u.test(value.recordId) ||
    !/^[0-9a-f]{64}$/u.test(value.hash) ||
    !['stdout', 'stderr', 'combined'].includes(value.stream) ||
    !['record-log', 'legacy-evidence', 'orphan-evidence'].includes(value.source) ||
    value.version !== 2 ||
    value.prefixByteLength > value.byteLength
  ) {
    throw new TypeError('recovered log frame is invalid');
  }
}

function parseRecoveredLogFrame(text: string): RecoveredLogFrame {
  const value = parseJson(text);
  assertRecoveredLogFrameValue(value);
  return value;
}

function assertStreamInventory(value: unknown): asserts value is StreamInventoryEntry[] {
  if (!Array.isArray(value)) throw new TypeError('stream inventory must be an array');
  for (const [index, entry] of value.entries()) {
    assertJsonObject(entry, `stream inventory entry ${String(index)}`);
    assertString(entry.name, `stream inventory entry ${String(index)} name`);
    assertNonnegativeInteger(entry.size, `stream inventory entry ${String(index)} size`);
    assertString(entry.sha256, `stream inventory entry ${String(index)} hash`);
  }
}

function parseStreamInventory(text: string): StreamInventoryEntry[] {
  const value = parseJson(text);
  assertStreamInventory(value);
  return value;
}

function assertEvidenceRootInspection(value: unknown): asserts value is EvidenceRootInspection {
  assertJsonObject(value, 'evidence root inspection');
  assertString(value.identity, 'evidence root inspection identity');
  assertStringArray(value.names, 'evidence root inspection names');
}

function parseEvidenceRootInspection(text: string): EvidenceRootInspection {
  const value = parseJson(text);
  assertEvidenceRootInspection(value);
  return value;
}

function recoveredLogFrames(stderr: string): RecoveredLogFrame[] {
  return stderr
    .split(/\r?\n/u)
    .filter((line) => line.startsWith('TQ_MACHINE_LOCK_RECOVERED_LOG:'))
    .map((line) => parseRecoveredLogFrame(line.slice('TQ_MACHINE_LOCK_RECOVERED_LOG:'.length)));
}

function assertRecoveredLogFrame(frame: RecoveredLogFrame) {
  expect(Object.keys(frame).sort()).toEqual([
    'byteLength',
    'contentPrefix',
    'hash',
    'prefixByteLength',
    'recordId',
    'source',
    'stream',
    'truncated',
    'version',
  ]);
  expect(frame.version).toBe(2);
  expect(['record-log', 'legacy-evidence', 'orphan-evidence']).toContain(frame.source);
  expect(frame.recordId).toMatch(/^[0-9a-f]{32}$/u);
  expect(['stdout', 'stderr', 'combined']).toContain(frame.stream);
  const prefix = Buffer.from(frame.contentPrefix, 'base64');
  expect(prefix).toHaveLength(frame.prefixByteLength);
  expect(frame.prefixByteLength).toBeLessThanOrEqual(64 * 1024);
  expect(frame.truncated).toBe(frame.byteLength > frame.prefixByteLength);
  if (!frame.truncated) {
    expect(createHash('sha256').update(prefix).digest('hex')).toBe(frame.hash);
  }
}

function runWrapper(command: string, environment: NodeJS.ProcessEnv = {}) {
  return spawnSync(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    windowsHide: true,
    timeout: wrapperTimeout,
  });
}
