import { createHash } from 'node:crypto';
import { mkdir, rm, symlink, writeFile } from 'node:fs/promises';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import {
  validateHostedRuntimeEvidence,
  verifyHostedRuntimeTree,
} from '../../scripts/windows-hosted-runtime-evidence.mjs';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const roots: string[] = [];
afterEach(async () => Promise.all(roots.splice(0).map(removeTestDirectory)));
const binding = {
  architecture: 'arm64',
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'b'.repeat(40),
  sourceTreeSha256: 'c'.repeat(64),
  version: '0.0.73',
  installer: 'setup.exe',
  installerSha256: 'd'.repeat(64),
  bytes: 123,
  provenanceDocumentSha256: 'e'.repeat(64),
  files: [
    {
      path: 'resources/role.exe',
      size: 3,
      sha256: createHash('sha256').update('abc').digest('hex'),
    },
  ],
};
const evidence = {
  ...binding,
  schemaVersion: 1,
  kind: 'github-hosted-native-runtime',
  result: 'passed',
  workflowRunId: '123',
  host: 'github-hosted',
  coverage: {
    installerPayload: 'verified',
    nativeRuntime: 'exercised',
    installation: 'not-exercised',
    uac: 'not-exercised',
  },
  runtimeFileCount: 1,
  runtimeVerifiedBefore: true,
  runtimeVerifiedAfter: true,
  lifecycle: {
    result: 'passed',
    architecture: 'arm64',
    mode: 'unpacked',
    first: { result: 'passed' },
    successor: { result: 'passed' },
    crash: { ownerAuthenticated: true },
    ownershipCoverage: { authoritative: false },
  },
};
describe('hosted exact native runtime smoke', () => {
  it('requires every payload file and rejects changed, extra or missing runtime files', async () => {
    const root = await createTestDirectory('hosted-runtime');
    roots.push(root);
    const runtime = join(root, 'unpacked');
    await mkdir(join(runtime, 'resources'), { recursive: true });
    const role = join(runtime, 'resources/role.exe');
    await writeFile(role, 'abc');
    await expect(verifyHostedRuntimeTree(runtime, binding)).resolves.toEqual({ fileCount: 1 });
    await writeFile(role, 'abd');
    await expect(verifyHostedRuntimeTree(runtime, binding)).rejects.toThrow(/differs/u);
    await writeFile(role, 'abc');
    await writeFile(join(runtime, 'extra.exe'), 'abc');
    await expect(verifyHostedRuntimeTree(runtime, binding)).rejects.toThrow(/inventory/u);
    await rm(join(runtime, 'extra.exe'));
    await rm(role);
    await expect(verifyHostedRuntimeTree(runtime, binding)).rejects.toThrow(/inventory/u);
  });
  it('rejects a reparse-point runtime root without traversing its target', async () => {
    const root = await createTestDirectory('hosted-runtime-reparse');
    roots.push(root);
    const target = join(root, 'target');
    await mkdir(target);
    const runtime = join(root, 'runtime');
    await symlink(target, runtime, process.platform === 'win32' ? 'junction' : 'dir');
    await expect(verifyHostedRuntimeTree(runtime, binding)).rejects.toThrow(/reparse/u);
  });

  it('binds native runtime coverage without allowing invented installation results', () => {
    expect(validateHostedRuntimeEvidence(evidence, binding)).toEqual(evidence);
    for (const field of [
      'architecture',
      'sourceCommit',
      'sourceTree',
      'sourceTreeSha256',
      'installerSha256',
      'provenanceDocumentSha256',
      'version',
      'installer',
      'bytes',
      'workflowRunId',
      'runtimeFileCount',
      'runtimeVerifiedBefore',
      'runtimeVerifiedAfter',
      'kind',
    ]) {
      expect(
        () => validateHostedRuntimeEvidence({ ...evidence, [field]: 'wrong' }, binding),
        field,
      ).toThrow();
    }
    for (const field of ['installation', 'uac', 'installerPayload', 'nativeRuntime']) {
      expect(() =>
        validateHostedRuntimeEvidence(
          { ...evidence, coverage: { ...evidence.coverage, [field]: 'wrong' } },
          binding,
        ),
      ).toThrow();
    }
    expect(() =>
      validateHostedRuntimeEvidence({ ...evidence, installExitCode: 0 }, binding),
    ).toThrow();
    expect(() =>
      validateHostedRuntimeEvidence({ ...evidence, uninstallExitCode: 0 }, binding),
    ).toThrow();
    expect(() =>
      validateHostedRuntimeEvidence(
        { ...evidence, lifecycle: { ...evidence.lifecycle, mode: 'installed' } },
        binding,
      ),
    ).toThrow();
  });
  it('runs the real unpacked lifecycle between complete integrity checks and retains optional installation diagnosis', () => {
    const script = readFileSync('scripts/windows-hosted-runtime-smoke.mjs', 'utf8');
    expect(script).toContain("'--mode'");
    expect(script).toContain("'unpacked'");
    expect(script).toContain('scripts/windows-package-lifecycle.mjs');
    expect(script.match(/await verifyHostedRuntimeTree/gu)).toHaveLength(2);
    expect(script).not.toContain('Start-Process');
    expect(script).not.toContain('installExitCode');
    const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
    expect(workflow).toContain('windows-hosted-runtime-smoke.mjs');
    expect(workflow).not.toContain('windows-hosted-lifecycle-smoke.ps1');
    const diagnostic = readFileSync(
      '.github/workflows/windows-hosted-launch-diagnostic.yml',
      'utf8',
    );
    expect(diagnostic).toContain('default: runtime');
    expect(diagnostic).toContain('windows-hosted-runtime-smoke.mjs');
    expect(diagnostic).toContain('windows-hosted-lifecycle-smoke.ps1');
    expect(diagnostic).toContain("default: '34013671889'");
  });
});
