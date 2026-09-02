import { spawnSync } from 'node:child_process';
import { existsSync, lstatSync, readFileSync, readdirSync, rmdirSync, unlinkSync } from 'node:fs';
import { resolve } from 'node:path';
import { describe, expect, it } from 'vitest';

const run = process.platform === 'win32' ? describe.sequential : describe.skip;
const wrapper = resolve('scripts', 'run-machine-lock-isolated-tests.mjs');
const records = resolve('tmp', 'machine-lock-tests', '.cleanup-records-v1');

run('Windows machine-lock wrapper teardown', () => {
  it.each(['roots-created', 'inventory-sealed', 'deleted-root:helper', 'registry-deleted'])(
    'recovers a durable record after a crash at %s',
    (phase) => {
      const crashed = runWrapper('cmd.exe /d /c exit 0', {
        TQ_MACHINE_LOCK_TEST_CRASH_AFTER: phase,
      });
      expect(crashed.status).toBe(197);
      expect(readdirSync(records)).toHaveLength(1);

      const recovered = runWrapper('cmd.exe /d /c exit 0');
      expect(recovered.status, recovered.stderr).toBe(0);
      expect(existsSync(records)).toBe(false);
    },
    120_000,
  );

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

function runWrapper(command: string, environment: NodeJS.ProcessEnv = {}) {
  return spawnSync(process.execPath, [wrapper, '--', command], {
    cwd: resolve('.'),
    env: { ...process.env, ...environment },
    encoding: 'utf8',
    windowsHide: true,
    timeout: 110_000,
  });
}
