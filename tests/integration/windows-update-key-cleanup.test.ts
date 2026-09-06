import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { currentSourceIdentity } from '../../scripts/source-identity.mjs';
import {
  protectSnapshot,
  verifySnapshotProtection,
} from '../../scripts/windows-update-native-chain.mjs';
import { sanitizedSubprocessEnvironment } from '../../scripts/environment-policy.mjs';

const root = resolve(`tmp/release-secrets/cleanup-integration-${String(process.pid)}`);
const keyPath = resolve(root, 'protected-key.pkcs8.der');
const descriptorPath = resolve(
  'tmp',
  `cleanup-integration-${String(process.pid)}`,
  'descriptor.json',
);
const descriptorArgument = `${resolve('tmp')}\\discarded-segment\\..\\cleanup-integration-${String(process.pid)}\\descriptor.json`;

const nativeHost = process.platform === 'win32' && process.arch === 'x64';
let cleanSource = false;
if (nativeHost) {
  try {
    currentSourceIdentity({ requireClean: true });
    cleanSource = true;
  } catch (error) {
    if (
      !(error instanceof Error) ||
      error.message !== 'Release provenance requires a clean source tree'
    ) {
      throw error;
    }
  }
}
// Key generation builds a provenance-bound native chain. Never bypass its clean-tree guard.
const nativeTest = nativeHost && cleanSource ? it : it.skip;

afterAll(() => {
  rmSync(root, { recursive: true, force: true });
  rmSync(resolve(descriptorPath, '..'), { recursive: true, force: true });
});

describe('Windows protected update-key failure cleanup', () => {
  it.skipIf(!nativeHost)(
    'replaces explicit snapshot ACEs without PATH or PATHEXT and reports only a safe rejection reason',
    () => {
      const snapshot = resolve(root, 'snapshot-secret-path');
      mkdirSync(snapshot, { recursive: true });
      writeFileSync(resolve(snapshot, 'receipt.json'), '{}');
      const path = process.env.PATH;
      const pathext = process.env.PATHEXT;
      try {
        // Hosted workspaces can supply explicit ACEs that /grant:r does not remove.
        for (const item of [snapshot, resolve(snapshot, 'receipt.json')]) {
          const seeded = spawnSync(
            resolve(process.env.SystemRoot ?? 'C:/Windows', 'System32/icacls.exe'),
            [item, '/grant', '*S-1-5-32-545:R'],
            { encoding: 'utf8', timeout: 30_000 },
          );
          expect(seeded.status, seeded.stderr).toBe(0);
        }
        process.env.PATH = resolve('tmp', 'tools-unavailable');
        delete process.env.PATHEXT;
        protectSnapshot(snapshot);
        expect(() => verifySnapshotProtection(snapshot)).not.toThrow();
        const changed = spawnSync(
          resolve(process.env.SystemRoot ?? 'C:/Windows', 'System32/icacls.exe'),
          [snapshot, '/grant', '*S-1-5-32-545:R'],
          { encoding: 'utf8', timeout: 30_000 },
        );
        expect(changed.status, changed.stderr).toBe(0);
        let message = '';
        try {
          verifySnapshotProtection(snapshot);
        } catch (error) {
          message = (error as Error).message;
        }
        expect(message).toContain('snapshot ace count');
        expect(message).not.toContain(snapshot);
        expect(message).not.toContain('snapshot-secret-path');
        message = '';
        try {
          verifySnapshotProtection(resolve(snapshot, 'missing-secret-path'));
        } catch (error) {
          message = (error as Error).message;
        }
        expect(message).toContain('snapshot inspection');
        expect(message).not.toContain('missing-secret-path');
      } finally {
        if (path === undefined) delete process.env.PATH;
        else process.env.PATH = path;
        if (pathext === undefined) delete process.env.PATHEXT;
        else process.env.PATHEXT = pathext;
        const retired = spawnSync(
          resolve(
            process.env.SystemRoot ?? 'C:/Windows',
            'System32/WindowsPowerShell/v1.0/powershell.exe',
          ),
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            String.raw`
$ErrorActionPreference='Stop'
$sid=[Security.Principal.WindowsIdentity]::GetCurrent().User.Value
& "$env:SystemRoot\System32\icacls.exe" $env:TQ_TEST_SNAPSHOT '/grant' "*$($sid):(OI)(CI)F" '/T' | Out-Null
exit $LASTEXITCODE
`,
          ],
          {
            env: sanitizedSubprocessEnvironment(process.env, { TQ_TEST_SNAPSHOT: snapshot }),
            encoding: 'utf8',
            timeout: 30_000,
          },
        );
        expect(retired.status, retired.stderr).toBe(0);
        rmSync(snapshot, { recursive: true, force: true });
      }
    },
    120_000,
  );

  it.skipIf(!nativeHost || cleanSource)(
    'rejects key generation from a dirty checkout before creating key material',
    () => {
      const generated = spawnSync(
        process.execPath,
        [
          'scripts/windows-update-native-chain.mjs',
          'key-generate',
          '--key-path',
          keyPath,
          '--descriptor',
          descriptorArgument,
        ],
        { cwd: resolve('.'), encoding: 'utf8', timeout: 30_000 },
      );
      expect(generated.error).toBeUndefined();
      expect(generated.status).toBe(1);
      expect(generated.stderr).toContain('Release provenance requires a clean source tree');
      expect(existsSync(keyPath)).toBe(false);
      expect(existsSync(descriptorPath)).toBe(false);
    },
  );

  nativeTest(
    'deletes through the retained descriptor after a forced producer failure with Cargo unavailable',
    () => {
      const generated = spawnSync(
        process.execPath,
        [
          'scripts/windows-update-native-chain.mjs',
          'key-generate',
          '--key-path',
          keyPath,
          '--descriptor',
          descriptorArgument,
        ],
        { cwd: resolve('.'), encoding: 'utf8', timeout: 12 * 60_000 },
      );
      expect(generated.status, generated.stderr).toBe(0);
      expect(existsSync(keyPath)).toBe(true);
      expect(existsSync(descriptorPath)).toBe(true);
      const generatedDescriptor = JSON.parse(generated.stdout) as { descriptorPath: string };
      expect(generatedDescriptor.descriptorPath).toBe(descriptorPath);

      const failedProducer = spawnSync(
        process.execPath,
        ['-e', "process.stderr.write('forced producer build failure');process.exit(91)"],
        { cwd: resolve('.'), encoding: 'utf8', timeout: 30_000 },
      );
      expect(failedProducer.status).toBe(91);
      expect(failedProducer.stderr).toContain('forced producer build failure');

      const cleanup = spawnSync(
        process.execPath,
        ['scripts/windows-update-native-chain.mjs', 'key-delete', '--descriptor', descriptorPath],
        {
          cwd: resolve('.'),
          env: {
            SystemRoot: process.env.SystemRoot ?? 'C:\\Windows',
            ProgramData: process.env.ProgramData ?? 'C:\\ProgramData',
            PATH: resolve('tmp', 'cargo-is-deliberately-unavailable'),
          },
          encoding: 'utf8',
          // ACL verification, native deletion and receipt retirement each have a 30s bound.
          timeout: 120_000,
        },
      );
      expect(cleanup.status, cleanup.stderr).toBe(0);
      expect(existsSync(keyPath)).toBe(false);
      expect(existsSync(descriptorPath)).toBe(false);
    },
    15 * 60_000,
  );
});
