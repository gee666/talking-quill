import { spawn, spawnSync } from 'node:child_process';
import { randomBytes } from 'node:crypto';
import {
  existsSync,
  linkSync,
  lstatSync,
  mkdirSync,
  readFileSync,
  readdirSync,
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
    'inventory-sealed',
    'deleted-root:helper',
    'binding-deleted-before-intent',
    'registry-deleted',
  ])(
    'recovers a durable record after a crash at %s',
    (phase) => {
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      });
      expect(crashed.status).toBe(197);
      expect(readdirSync(records).filter((entry) => entry.endsWith('.json'))).toHaveLength(1);

      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

  it.each(['hardlink', 'reparse'] as const)(
    'rejects a post-seal %s and preserves its outside target',
    (kind) => {
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'inventory-sealed',
      });
      expect(crashed.status).toBe(197);
      const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
      const record = JSON.parse(readFileSync(resolve(records, recordName), 'utf8'));
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

  it('blocks root rename and replacement between identity and ADS publication', async () => {
    const token = randomBytes(16).toString('hex');
    const pause = resolve('tmp', 'machine-lock-wrapper-tests', `${token}-root-publication`);
    mkdirSync(resolve(pause, '..'), { recursive: true });
    const running = runWrapperAsync('cmd.exe /d /c exit 0', {
      TQ_MACHINE_LOCK_TEST_ROOT_PUBLICATION_PAUSE_FILE: pause,
    });
    await waitForPath(`${pause}.ready`);
    const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
    const record = JSON.parse(readFileSync(resolve(records, recordName), 'utf8'));
    const root = resolve('tmp', 'machine-lock-tests', record.creatingRoot, record.namespaceId);
    const moved = `${root}-moved`;
    const attacker = `${root}-replacement`;
    mkdirSync(attacker);
    let renameBlocked = false;
    try {
      renameSync(root, moved);
    } catch {
      renameBlocked = true;
    }
    const replacement = spawnSync(nativeHelper, ['--force-directory-replacement', attacker, root]);
    const attackerPreserved = existsSync(attacker);
    const rootPreserved = existsSync(root);
    if (existsSync(attacker)) rmdirSync(attacker);
    writeFileSync(`${pause}.continue`, 'continue\n', 'utf8');
    const completed = await running;
    if (existsSync(moved) && !existsSync(root)) renameSync(moved, root);
    unlinkSync(`${pause}.ready`);
    unlinkSync(`${pause}.continue`);
    expect(renameBlocked).toBe(true);
    expect(existsSync(moved)).toBe(false);
    expect(replacement.status, replacement.stderr?.toString()).toBe(0);
    expect(attackerPreserved).toBe(true);
    expect(rootPreserved).toBe(true);
    expect(completed.code, completed.stderr).toBe(0);
    const parent = resolve(pause, '..');
    if (readdirSync(parent).length === 0) rmdirSync(parent);
  }, 120_000);

  it('retains namespace root and parent handles while the child runs', () => {
    const result = runWrapper('node tests/fixtures/machine-lock-test-retained-parent.mjs');
    expect(result.status, result.stderr).toBe(0);
  }, 120_000);

  it('recovers a legacy cleanup record without parent identities', () => {
    const record = leaveSealedRecord();
    record.schemaVersion = 1;
    for (const root of record.roots) delete root.parentIdentity;
    writeFileSync(
      resolve(records, `${record.recordId}.json`),
      `${JSON.stringify(record)}\n`,
      'utf8',
    );
    expect(runWrapper('cmd.exe /d /c exit 0').status).toBe(0);
  }, 120_000);

  it('rejects an ownership ADS mutation after sealing', () => {
    const record = leaveSealedRecord();
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
    const record = leaveSealedRecord();
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

  it('rejects and preserves an unrecorded registry sibling', () => {
    leaveSealedRecord();
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

function leaveSealedRecord() {
  const crashed = runWrapper('cmd.exe /d /c exit 0', {
    TQ_MACHINE_LOCK_TEST_CRASH_AFTER: 'inventory-sealed',
  });
  expect(crashed.status).toBe(197);
  const recordName = readdirSync(records).find((name) => name.endsWith('.json'))!;
  return JSON.parse(readFileSync(resolve(records, recordName), 'utf8'));
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

function runWrapper(command: string, environment: NodeJS.ProcessEnv = {}) {
  return spawnSync(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 110_000,
  });
}
