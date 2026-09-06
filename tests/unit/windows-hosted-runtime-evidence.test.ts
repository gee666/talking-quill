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
  schemaVersion: 2,
  kind: 'github-hosted-native-runtime',
  result: 'passed',
  workflowRunId: '123',
  host: 'github-hosted',
  coverage: {
    installerPayload: 'verified',
    nativeRuntime: 'startup-observed',
    ownerAuthentication: 'not-observed',
    transactions: 'not-exercised',
    gracefulLifecycle: 'not-asserted',
    installation: 'not-exercised',
    uac: 'not-exercised',
  },
  runtimeFileCount: 1,
  runtimeVerifiedBefore: true,
  runtimeVerifiedAfter: true,
  startup: {
    schemaVersion: 1,
    kind: 'windows-production-startup-observation',
    result: 'passed',
    failure: null,
    architecture: 'arm64',
    mode: 'unpacked',
    launch: { arguments: [], testHooks: false, profile: 'fresh-hosted-default' },
    coverage: {
      startup: 'observed',
      ownerAuthentication: 'not-observed',
      transactions: 'not-exercised',
      gracefulLifecycle: 'not-asserted',
    },
    observation: {
      mainPid: 100,
      stableSamples: 2,
      window: {
        pid: 100,
        visible: true,
        title: 'Talking Quill',
        width: 900,
        height: 600,
        accessibilitySource: 'Windows.UIAutomation',
        markers: ['Talking Quill', 'Welcome', 'Continue'],
        contentElementCount: 20,
      },
      helper: {
        pid: 101,
        parentPid: 100,
        relativePath: 'resources/helper/talking-quill-helper.exe',
      },
      owner: {
        pid: 102,
        parentPid: 101,
        relativePath: 'resources/helper/talking-quill-keyboard-owner.exe',
      },
      rendererPids: [103],
    },
    cleanup: {
      closeRequested: true,
      forcedTermination: true,
      forcedPids: [100, 101, 102, 103],
      remainingPackageProcesses: 0,
    },
    durationMs: 5000,
    screenshot: 'startup-window.png',
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
        { ...evidence, startup: { ...evidence.startup, mode: 'installed' } },
        binding,
      ),
    ).toThrow();
  });
  it('rejects process-only startup, test hooks, invented authentication and incomplete cleanup', () => {
    const mutations: [string, unknown][] = [
      ['schemaVersion', 1],
      ['lifecycle', { result: 'passed' }],
      ['coverage.ownerAuthentication', 'authenticated'],
      ['coverage.transactions', 'exercised'],
      ['coverage.gracefulLifecycle', 'passed'],
      ['startup.result', 'failed'],
      ['startup.failure', 'startup failed'],
      ['startup.launch.arguments', ['--talking-quill-installed-readiness-pipe=fake']],
      ['startup.launch.testHooks', true],
      ['startup.launch.profile', 'existing'],
      ['startup.coverage.ownerAuthentication', 'authenticated'],
      ['startup.coverage.transactions', 'exercised'],
      ['startup.coverage.gracefulLifecycle', 'passed'],
      ['startup.observation.window.visible', false],
      ['startup.observation.window.markers', ['Talking Quill']],
      ['startup.observation.window.accessibilitySource', 'process-title'],
      ['startup.observation.window.contentElementCount', 0],
      ['startup.observation.window.pid', 999],
      ['startup.observation.stableSamples', 1],
      ['startup.observation.helper.relativePath', 'other/helper.exe'],
      ['startup.observation.owner.relativePath', 'other/owner.exe'],
      ['startup.observation.owner.pid', 0],
      ['startup.observation.owner.ownerAuthenticated', true],
      ['startup.observation.rendererPids', []],
      ['startup.observation.rendererPids', [100]],
      ['startup.cleanup.remainingPackageProcesses', 1],
      ['startup.cleanup.forcedTermination', false],
      ['startup.cleanup.error', 'cleanup failed'],
      ['startup.screenshot', null],
    ];
    for (const [path, value] of mutations) {
      const mutated = structuredClone(evidence) as unknown as Record<string, unknown>;
      const keys = path.split('.');
      let target = mutated;
      for (const key of keys.slice(0, -1)) target = target[key] as Record<string, unknown>;
      const leaf = keys.at(-1);
      if (leaf === undefined) throw new Error('Invalid test mutation');
      target[leaf] = value;
      expect(() => validateHostedRuntimeEvidence(mutated, binding), path).toThrow();
    }
  });
  it.each(['x64', 'arm64'])(
    'binds observed startup to the native %s architecture',
    (architecture) => {
      expect(
        validateHostedRuntimeEvidence(
          {
            ...evidence,
            architecture,
            startup: { ...evidence.startup, architecture },
          },
          { ...binding, architecture },
        ),
      ).toBeTruthy();
      expect(() =>
        validateHostedRuntimeEvidence(
          { ...evidence, architecture, startup: { ...evidence.startup, architecture: 'wrong' } },
          { ...binding, architecture },
        ),
      ).toThrow();
    },
  );
  it('allows either observed cleanup outcome without turning it into lifecycle coverage', () => {
    const startup = {
      ...evidence.startup,
      cleanup: {
        closeRequested: true,
        forcedTermination: false,
        forcedPids: [],
        remainingPackageProcesses: 0,
      },
    };
    expect(validateHostedRuntimeEvidence({ ...evidence, startup }, binding)).toBeTruthy();
  });
  it('runs production startup observation between complete integrity checks and retains optional installation diagnosis', () => {
    const script = readFileSync('scripts/windows-hosted-runtime-smoke.mjs', 'utf8');
    expect(script).toContain('scripts/windows-production-startup-smoke.ps1');
    expect(script).not.toContain("'--mode'");
    expect(script).not.toContain('talking-quill-installed-readiness');
    expect(script).toContain("nativeRuntime: 'startup-observed'");
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
