import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { sanitizedSubprocessEnvironment } from '../../scripts/environment-policy.mjs';
import {
  cleanupAcceptanceNative,
  cleanupAcceptanceNativeDescriptor,
  publishAcceptanceNative,
} from '../../scripts/windows-installed-acceptance-native-publication.mjs';

const root = resolve(`tmp/native-publication-integration-${String(process.pid)}`);
const target = resolve('helper/target/x86_64-pc-windows-msvc/release');
const source = resolve(root, 'source');
const elevated =
  process.platform === 'win32' &&
  process.arch === 'x64' &&
  spawnSync('net.exe', ['session'], { windowsHide: true, stdio: 'ignore' }).status === 0;
const nativeTest = elevated ? it : it.skip;

beforeAll(() => {
  if (!elevated) return;
  rmSync(root, { recursive: true, force: true });
  mkdirSync(source, { recursive: true });
  const sourceCommit = spawnSync('git', ['rev-parse', 'HEAD'], { encoding: 'utf8' }).stdout.trim();
  const sourceTree = spawnSync('git', ['rev-parse', 'HEAD^{tree}'], {
    encoding: 'utf8',
  }).stdout.trim();
  const common = [
    'build',
    '--manifest-path',
    'helper/Cargo.toml',
    '--locked',
    '--release',
    '--target',
    'x86_64-pc-windows-msvc',
  ];
  const environment = {
    ...process.env,
    TALKING_QUILL_SOURCE_COMMIT: sourceCommit,
    TALKING_QUILL_SOURCE_TREE: sourceTree,
  };
  for (const arguments_ of [
    [...common, '-p', 'talking-quill-acceptance-signer'],
    [...common, '-p', 'talking-quill-helper', '--features', 'windows-installed-acceptance'],
  ]) {
    const result = spawnSync('cargo.exe', arguments_, {
      env: environment,
      encoding: 'utf8',
      windowsHide: true,
      timeout: 12 * 60_000,
      maxBuffer: 16 * 1024,
    });
    expect(result.status, result.stderr).toBe(0);
  }
  for (const name of [
    'talking-quill-acceptance-signer.exe',
    'talking-quill-windows-acceptance-broker.exe',
    'talking-quill-helper.exe',
  ]) {
    copyFileSync(resolve(target, name), resolve(source, name));
  }
}, 25 * 60_000);

afterAll(() => rmSync(root, { recursive: true, force: true }));

describe('Windows acceptance native publication', () => {
  it('requires an absolute cleanup descriptor path', async () => {
    await expect(
      cleanupAcceptanceNativeDescriptor('relative.json', { descriptorSha256: '0'.repeat(64) }),
    ).rejects.toThrow('Native publication descriptor path must be absolute');
  });

  nativeTest(
    'applies exact per-root ACLs and performs identity-bound cleanup',
    async () => {
      const outputRoot = resolve(root, 'successful-output');
      mkdirSync(outputRoot, { recursive: true });
      const base = resolve(root, 'program-data/Talking Quill Acceptance Native');
      mkdirSync(base, { recursive: true });
      const before = readFileSyncAcl(base);
      const descriptor = await publishAcceptanceNative({
        buildId: 'a'.repeat(64),
        sourceRoot: source,
        outputRoot,
        programData: resolve(root, 'program-data'),
      });
      expect(readFileSyncAcl(base)).toBe(before);
      expect(descriptor.inventory).toHaveLength(3);
      expect(descriptor.publicationId).toMatch(/^[0-9a-f]{64}$/u);
      expect(descriptor.publicationId).not.toBe(descriptor.buildId);
      expect(descriptor.nativeRoot).toBe(resolve(base, descriptor.publicationId));
      expect(
        spawnSync('icacls.exe', [descriptor.nativeRoot, '/grant', '*S-1-5-32-545:R'], {
          encoding: 'utf8',
          windowsHide: true,
        }).status,
      ).toBe(0);
      await expect(
        cleanupAcceptanceNative(descriptor, {
          programData: resolve(root, 'program-data'),
        }),
      ).rejects.toThrow('Native publication ACL verify failed');
      expect(existsSync(descriptor.nativeRoot)).toBe(true);
      expect(
        spawnSync('icacls.exe', [descriptor.nativeRoot, '/remove:g', '*S-1-5-32-545'], {
          encoding: 'utf8',
          windowsHide: true,
        }).status,
      ).toBe(0);
      await cleanupAcceptanceNativeDescriptor(descriptor.descriptorPath, {
        descriptorSha256: descriptor.descriptorSha256,
        programData: resolve(root, 'program-data'),
      });
      expect(existsSync(descriptor.nativeRoot)).toBe(false);
      expect(existsSync(descriptor.cleanupLauncher.path)).toBe(false);
      expect(existsSync(descriptor.descriptorPath)).toBe(false);
      expect(existsSync(base)).toBe(true);
      expect(readFileSyncAcl(base)).toBe(before);
    },
    25 * 60_000,
  );

  nativeTest(
    'removes an exact partial publication when copying fails',
    async () => {
      const broken = resolve(root, 'broken-source');
      const outputRoot = resolve(root, 'failed-output');
      mkdirSync(broken, { recursive: true });
      mkdirSync(outputRoot, { recursive: true });
      copyFileSync(
        resolve(source, 'talking-quill-acceptance-signer.exe'),
        resolve(broken, 'talking-quill-acceptance-signer.exe'),
      );
      copyFileSync(
        resolve(source, 'talking-quill-helper.exe'),
        resolve(broken, 'talking-quill-helper.exe'),
      );
      await expect(
        publishAcceptanceNative({
          buildId: 'b'.repeat(64),
          sourceRoot: broken,
          outputRoot,
          programData: resolve(root, 'program-data'),
        }),
      ).rejects.toThrow();
      expect(readdirSync(resolve(root, 'program-data/Talking Quill Acceptance Native'))).toEqual(
        [],
      );
    },
    120_000,
  );

  nativeTest(
    'removes the exact failed snapshot and its newly created base',
    async () => {
      const programData = resolve(root, 'fresh-program-data');
      const outputRoot = resolve(root, 'fresh-failed-output');
      const broken = resolve(root, 'fresh-broken-source');
      mkdirSync(programData, { recursive: true });
      mkdirSync(outputRoot, { recursive: true });
      mkdirSync(broken, { recursive: true });
      for (const name of ['talking-quill-acceptance-signer.exe', 'talking-quill-helper.exe']) {
        copyFileSync(resolve(source, name), resolve(broken, name));
      }
      await expect(
        publishAcceptanceNative({
          buildId: 'c'.repeat(64),
          sourceRoot: broken,
          outputRoot,
          programData,
        }),
      ).rejects.toThrow();
      expect(existsSync(resolve(programData, 'Talking Quill Acceptance Native'))).toBe(false);
    },
    120_000,
  );
});

function readFileSyncAcl(path: string): string {
  const result = spawnSync(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-Command', '(Get-Acl -LiteralPath $env:TQ_TEST_ACL).Sddl'],
    {
      env: sanitizedSubprocessEnvironment(process.env, { TQ_TEST_ACL: path }),
      encoding: 'utf8',
      windowsHide: true,
    },
  );
  expect(result.status, result.stderr).toBe(0);
  return result.stdout.trim();
}
