import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { validateHostedLifecycleEvidence } from '../../scripts/windows-hosted-lifecycle-evidence.mjs';

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
  files: [{ path: 'Talking Quill.exe', size: 1, sha256: 'f'.repeat(64) }],
};
const evidence = {
  ...binding,
  schemaVersion: 1,
  kind: 'github-hosted-elevated-install-lifecycle',
  result: 'passed',
  workflowRunId: '123',
  host: 'github-hosted',
  elevated: true,
  uac: 'not-exercised',
  machineStateBefore: 'absent',
  machineStateAfter: 'absent',
  installExitCode: 0,
  uninstallExitCode: 0,
  installedFileCount: 1,
  installedFilesVerified: true,
  lifecycle: {
    result: 'passed',
    architecture: 'arm64',
    mode: 'installed',
    first: { result: 'passed' },
    successor: { result: 'passed' },
    crash: { ownerAuthenticated: true },
    ownershipCoverage: { authoritative: false },
  },
};
describe('honest elevated hosted lifecycle evidence', () => {
  it('binds actual install, authenticated owner lifecycle and removal without asserting UAC', () => {
    expect(validateHostedLifecycleEvidence(evidence, binding)).toEqual(evidence);
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
      'machineStateBefore',
      'machineStateAfter',
      'installExitCode',
      'uninstallExitCode',
      'installedFilesVerified',
      'installedFileCount',
      'elevated',
      'uac',
      'kind',
    ]) {
      expect(
        () => validateHostedLifecycleEvidence({ ...evidence, [field]: 'wrong' }, binding),
        field,
      ).toThrow();
    }
    expect(() =>
      validateHostedLifecycleEvidence(
        { ...evidence, lifecycle: { ...evidence.lifecycle, crash: { ownerAuthenticated: false } } },
        binding,
      ),
    ).toThrow();
    expect(() =>
      validateHostedLifecycleEvidence(
        { ...evidence, lifecycle: { ...evidence.lifecycle, successor: { result: 'failed' } } },
        binding,
      ),
    ).toThrow();
  });
  it('refuses local/non-elevated/existing installs and uses exact maintenance registration', () => {
    const script = readFileSync('scripts/windows-hosted-lifecycle-smoke.ps1', 'utf8');
    expect(script).toContain("$env:RUNNER_ENVIRONMENT -cne 'github-hosted'");
    expect(script).toContain('WindowsBuiltInRole]::Administrator');
    expect(script).toContain('$before.Count -ne 0');
    expect(script.indexOf('$before.Count -ne 0')).toBeLessThan(
      script.indexOf("Run-Quiet $Installer 'install'"),
    );
    expect(script).toContain('[TqHostedMediumLauncher]::Launch(');
    expect(script).not.toContain('Start-Process -FilePath $Executable');
    const launcher = readFileSync('scripts/windows-hosted-medium-launcher.cs', 'utf8');
    expect(launcher).toContain('CreateProcessWithTokenW(primary');
    expect(launcher).toContain('candidate.integrity != 0x2000 || candidate.elevated != 0');
    expect(launcher).toContain('SameIdentity(ReadClaims(linked), host)');
    expect(launcher).toContain('GetShellWindow()');
    expect(launcher).toContain('No authenticated medium token is available.');
    expect(launcher).not.toContain('CreateRestrictedToken');
    expect(launcher).not.toContain('SetTokenInformation');
    expect(script).toContain('QuietUninstallString');
    expect(script).toContain('Maintenance identity changed; refusing cleanup.');
    expect(script).toContain('--mode installed');
    expect(script).toContain('Installed file differs from authenticated native package');
    expect(script).toContain('Machine removal incomplete');
    expect(script).toContain("uac='not-exercised'");
    expect(script).not.toContain('Remove-Item');
    expect(script).not.toContain('Stop-Process');
    const workflow = readFileSync('.github/workflows/release-unsigned.yml', 'utf8');
    expect(workflow).toContain(
      'Upload hosted lifecycle logs even on failure\n        if: always()',
    );
    expect(workflow).not.toContain('run-windows-installer-ui-smoke.mjs');
  });
});
