import { existsSync, rmSync } from 'node:fs';
import { spawnSync } from 'node:child_process';
import { resolve } from 'node:path';
import { afterAll, describe, expect, it } from 'vitest';
import { currentSourceIdentity } from '../../scripts/source-identity.mjs';

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
          timeout: 30_000,
        },
      );
      expect(cleanup.status, cleanup.stderr).toBe(0);
      expect(existsSync(keyPath)).toBe(false);
      expect(existsSync(descriptorPath)).toBe(false);
    },
    13 * 60_000,
  );
});
