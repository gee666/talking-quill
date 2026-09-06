import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, readdirSync, rmSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { sanitizedSubprocessEnvironment } from '../../scripts/environment-policy.mjs';
import {
  cleanupAcceptanceNative,
  cleanupAcceptanceNativeDescriptor,
  publishAcceptanceNative,
  runAclScript,
} from '../../scripts/windows-installed-acceptance-native-publication.mjs';

const root = resolve(`tmp/native-publication-integration-${String(process.pid)}`);
const target = resolve('helper/target/x86_64-pc-windows-msvc/release');
const source = resolve(root, 'source');
const elevated =
  process.platform === 'win32' &&
  process.arch === 'x64' &&
  spawnSync('net.exe', ['session'], { windowsHide: true, stdio: 'ignore' }).status === 0;
const nativeTest = elevated ? it : it.skip;

function buildNativeSource() {
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
}

afterAll(() => rmSync(root, { recursive: true, force: true }));

describe('Windows acceptance native publication', () => {
  beforeAll(buildNativeSource, 25 * 60_000);
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
      const programData = resolve(root, 'partial-program-data');
      const base = resolve(programData, 'Talking Quill Acceptance Native');
      mkdirSync(base, { recursive: true });
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
          programData,
        }),
      ).rejects.toThrow();
      expect(readdirSync(base)).toEqual([]);
    },
    8 * 60_000,
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
    8 * 60_000,
  );
});

describe('Windows acceptance publication ACL regression', () => {
  nativeTest(
    'enforces actual SID rights despite hostile PowerShell module environment',
    () => {
      const snapshot = resolve(root, 'acl-only');
      const moduleRoot = resolve(root, 'hostile-modules');
      const modulePath = resolve(moduleRoot, 'Microsoft.PowerShell.Security');
      mkdirSync(snapshot, { recursive: true });
      mkdirSync(modulePath, { recursive: true });
      writeFileSync(
        resolve(modulePath, 'Microsoft.PowerShell.Security.psm1'),
        "throw 'hostile module loaded'; function Get-Acl { throw 'hostile Get-Acl' }; function Set-Acl { throw 'hostile Set-Acl' }",
      );
      const previousModulePath = process.env.PSModulePath;
      const previousCache = process.env.PSModuleAnalysisCachePath;
      try {
        process.env.PSModulePath = moduleRoot;
        process.env.PSModuleAnalysisCachePath = resolve(moduleRoot, 'hostile-cache');
        const sid = runAclScript(snapshot, 'initialize');
        expect(runAclScript(snapshot, 'verify-initial', sid)).toBe(sid);
        const child = resolve(snapshot, 'receipt.json');
        writeFileSync(child, '{}');
        for (const path of [snapshot, child]) {
          const seeded = spawnSync(
            resolve(process.env.SystemRoot ?? 'C:/Windows', 'System32/icacls.exe'),
            [path, '/grant', '*S-1-5-32-545:R'],
            { encoding: 'utf8', timeout: 30_000 },
          );
          expect(seeded.status, seeded.stderr).toBe(0);
        }
        expect(runAclScript(snapshot, 'protect', sid)).toBe(sid);
        expect(runAclScript(snapshot, 'verify', sid)).toBe(sid);
        const inspected = spawnSync(
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
$items=@(Get-Item -LiteralPath $env:TQ_TEST_ACL)+@(Get-ChildItem -LiteralPath $env:TQ_TEST_ACL -Force)
@($items | ForEach-Object {
  $acl=$_.GetAccessControl([Security.AccessControl.AccessControlSections]'Owner, Access')
  @{
    owner=$acl.GetOwner([Security.Principal.SecurityIdentifier]).Value
    protected=$acl.AreAccessRulesProtected
    rules=@($acl.GetAccessRules($true,$true,[Security.Principal.SecurityIdentifier]) | ForEach-Object {
      @{sid=$_.IdentityReference.Value;rights=[int]$_.FileSystemRights;inheritance=[int]$_.InheritanceFlags;propagation=[int]$_.PropagationFlags;inherited=$_.IsInherited;type=[int]$_.AccessControlType}
    })
  }
}) | ConvertTo-Json -Depth 5 -Compress
`,
          ],
          {
            env: sanitizedSubprocessEnvironment(process.env, { TQ_TEST_ACL: snapshot }),
            encoding: 'utf8',
            timeout: 120_000,
          },
        );
        expect(inspected.status, inspected.stderr).toBe(0);
        const acls = JSON.parse(inspected.stdout) as {
          owner: string;
          protected: boolean;
          rules: {
            sid: string;
            rights: number;
            inheritance: number;
            propagation: number;
            inherited: boolean;
            type: number;
          }[];
        }[];
        const expected = new Map([
          [sid, 1179817],
          ['S-1-5-18', 2032127],
          ['S-1-5-32-544', 2032127],
        ]);
        expect(acls).toHaveLength(2);
        for (const acl of acls) {
          expect(acl.owner).toBe('S-1-5-32-544');
          expect(acl.protected).toBe(true);
          expect(acl.rules).toHaveLength(expected.size);
          expect(new Set(acl.rules.map((rule) => rule.sid)).size).toBe(expected.size);
          for (const rule of acl.rules) {
            expect(rule.rights).toBe(expected.get(rule.sid));
            expect(rule.inheritance).toBe(0);
            expect(rule.propagation).toBe(0);
            expect(rule.inherited).toBe(false);
            expect(rule.type).toBe(0);
          }
        }
        const changed = spawnSync(
          resolve(process.env.SystemRoot ?? 'C:/Windows', 'System32/icacls.exe'),
          [child, '/grant', '*S-1-5-32-545:R'],
          { encoding: 'utf8', timeout: 30_000 },
        );
        expect(changed.status, changed.stderr).toBe(0);
        expect(() => runAclScript(snapshot, 'verify', sid)).toThrow(
          'Native publication ACL verify failed',
        );
        expect(existsSync(child)).toBe(true);
      } finally {
        if (previousModulePath === undefined) delete process.env.PSModulePath;
        else process.env.PSModulePath = previousModulePath;
        if (previousCache === undefined) delete process.env.PSModuleAnalysisCachePath;
        else process.env.PSModuleAnalysisCachePath = previousCache;
        rmSync(snapshot, { recursive: true, force: true });
      }
    },
    15 * 60_000,
  );
});

function readFileSyncAcl(path: string): string {
  const result = spawnSync(
    'powershell.exe',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      '(Get-Item -LiteralPath $env:TQ_TEST_ACL).GetAccessControl().GetSecurityDescriptorSddlForm([Security.AccessControl.AccessControlSections]::All)',
    ],
    {
      env: sanitizedSubprocessEnvironment(process.env, { TQ_TEST_ACL: path }),
      encoding: 'utf8',
      windowsHide: true,
      timeout: 120_000,
    },
  );
  expect(result.status, result.stderr).toBe(0);
  return result.stdout.trim();
}
