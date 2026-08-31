import { describe, expect, it } from 'vitest';
import {
  validateWindowsInstallerUiEvidence,
  WINDOWS_INSTALLER_UI_CANCELLATION_EXIT_CODE,
} from '../../scripts/windows-installer-ui-evidence.mjs';
import { createWindowsInstallerUiSmokePlan } from '../../scripts/run-windows-installer-ui-smoke.mjs';

const expected = {
  installer: 'Talking-Quill-0.0.69-win-x64.exe',
  architecture: 'x64' as const,
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'b'.repeat(40),
  sourceTreeSha256: 'd'.repeat(64),
  installerSha256: 'c'.repeat(64),
  provenanceDocumentSha256: 'e'.repeat(64),
  bytes: 100,
};

function evidence() {
  return {
    schemaVersion: 2,
    installer: expected.installer,
    architecture: expected.architecture,
    sourceCommit: expected.sourceCommit,
    sourceTree: expected.sourceTree,
    sourceTreeSha256: expected.sourceTreeSha256,
    installerProvenanceSha256: expected.installerSha256,
    provenanceDocumentSha256: expected.provenanceDocumentSha256,
    bytes: expected.bytes,
    installerSha256Before: expected.installerSha256,
    installerSha256After: expected.installerSha256,
    outerPeSubsystem: 'windows-gui',
    outerPeSubsystemValue: 2,
    nsisWindow: { title: 'Talking Quill Setup', className: '#32770', processId: 42 },
    monitoring: {
      sampleIntervalMs: 5,
      maximumSampleGapMs: 10,
      processSamples: 20,
      windowSamples: 20,
      filesystemSamples: 20,
      registrySamples: 20,
      errors: [],
      events: [
        '1ms launched outer pid=40',
        '20ms classified pid=41 role=elevated-bootstrap',
        '40ms classified pid=42 role=inner-nsis',
        '45ms IDCANCEL hwnd=100 pid=42 accepted=True',
      ],
    },
    processes: [
      { pid: 40, parentPid: 1, image: expected.installer, role: 'outer-bootstrap', exitCode: 0 },
      {
        pid: 41,
        parentPid: 40,
        image: expected.installer,
        role: 'elevated-bootstrap',
        exitCode: 0,
      },
      {
        pid: 42,
        parentPid: 41,
        image: 'Talking-Quill-inner-installer.exe',
        role: 'inner-nsis',
        exitCode: 0,
      },
    ],
    installerRoleExits: {
      'outer-bootstrap': { pid: 40, exitCode: 0 },
      'elevated-bootstrap': { pid: 41, exitCode: 0 },
      'inner-nsis': { pid: 42, exitCode: 0 },
    },
    powershellProcessStarts: 0,
    visibleConsoleWindowEvents: [],
    filesystemOrRegistryMutationEvents: [],
    transientProtectedBootstrapObserved: true,
    protectedBootstrapBaselineRestored: true,
    cancellation: {
      method: 'WM_COMMAND/IDCANCEL',
      targetRole: 'inner-nsis',
      targetProcessId: 42,
      postAccepted: true,
      confirmationObserved: true,
      confirmationProcessId: 42,
      confirmationPostAccepted: true,
      graceful: true,
      forcedCleanup: false,
      exitCode: WINDOWS_INSTALLER_UI_CANCELLATION_EXIT_CODE,
    },
    activeProcessesAfterTeardown: [],
    noDurableInstallMutation: true,
    exactBaselineRestored: true,
    passed: true,
  };
}

describe('mandatory Windows installer UI smoke evidence', () => {
  it('accepts exact GUI cancellation evidence and resolves local exact artifacts', () => {
    expect(validateWindowsInstallerUiEvidence(evidence(), expected)).toEqual(evidence());
    const plan = createWindowsInstallerUiSmokePlan({
      architecture: 'arm64',
      version: '0.0.69',
      variant: 'canonical',
    });
    expect(plan.architecture).toBe('arm64');
    expect(plan.installer).toContain('Talking-Quill-0.0.69-win-arm64.exe');
    expect(plan.provenance).toContain('artifact-provenance.json');
    expect(plan.output).toContain('windows-installer-ui-smoke-arm64.json');
  });

  it.each([
    ['artifact hash substitution', { installerSha256After: 'd'.repeat(64) }],
    ['wrong source tree', { sourceTree: 'd'.repeat(40) }],
    ['wrong provenance document', { provenanceDocumentSha256: 'f'.repeat(64) }],
    ['surviving child process', { activeProcessesAfterTeardown: [42] }],
    ['transient console flash', { visibleConsoleWindowEvents: ['hook:41:ConsoleWindowClass:'] }],
    [
      'transient registry create/delete',
      { filesystemOrRegistryMutationEvents: ['registry snapshot changed'] },
    ],
    [
      'transient Program Files create/delete',
      { filesystemOrRegistryMutationEvents: ['Created:C:/Program Files/Talking Quill'] },
    ],
    [
      'monitor overflow',
      { monitoring: { ...evidence().monitoring, errors: ['filesystem watcher overflow'] } },
    ],
    ['sampling gap', { monitoring: { ...evidence().monitoring, maximumSampleGapMs: 51 } }],
    [
      'outer window selected',
      { cancellation: { ...evidence().cancellation, targetProcessId: 40 } },
    ],
    [
      'wrong protected role binding',
      {
        installerRoleExits: {
          ...evidence().installerRoleExits,
          'inner-nsis': { pid: 43, exitCode: 0 },
        },
      },
    ],
    [
      'elevated wrapper nonzero exit',
      {
        installerRoleExits: {
          ...evidence().installerRoleExits,
          'elevated-bootstrap': { pid: 41, exitCode: 124 },
        },
      },
    ],
    [
      'elevated role not bound to process record',
      {
        processes: evidence().processes.map((process) =>
          process.pid === 41 ? { ...process, role: null } : process,
        ),
      },
    ],
    [
      'duplicate NSIS role PID',
      {
        installerRoleExits: {
          ...evidence().installerRoleExits,
          'elevated-bootstrap': { pid: 40, exitCode: 0 },
        },
      },
    ],
    [
      'cancel delivery not accepted',
      { cancellation: { ...evidence().cancellation, postAccepted: false } },
    ],
    [
      'observed confirmation not accepted',
      { cancellation: { ...evidence().cancellation, confirmationPostAccepted: false } },
    ],
    ['forced cleanup', { cancellation: { ...evidence().cancellation, forcedCleanup: true } }],
    ['missing graceful exit', { cancellation: { ...evidence().cancellation, graceful: false } }],
    [
      'generic nonzero cancellation exit',
      { cancellation: { ...evidence().cancellation, exitCode: 1 } },
    ],
    [
      'protected bootstrap rejection exit',
      { cancellation: { ...evidence().cancellation, exitCode: 78 } },
    ],
    [
      'protected bootstrap launch exit',
      { cancellation: { ...evidence().cancellation, exitCode: 79 } },
    ],
    ['protected leaf residue', { protectedBootstrapBaselineRestored: false }],
  ])('rejects %s', (_label, patch) => {
    expect(() => validateWindowsInstallerUiEvidence({ ...evidence(), ...patch }, expected)).toThrow(
      'not promotable',
    );
  });
});
