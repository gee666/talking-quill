import { describe, expect, it } from 'vitest';
import { validateWindowsInstallerUiEvidence } from '../../scripts/windows-installer-ui-evidence.mjs';

const hash = '1'.repeat(64);
const expected = {
  installer: 'Talking-Quill-0.0.69-win-x64.exe',
  architecture: 'x64',
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'b'.repeat(40),
  sourceTreeSha256: '2'.repeat(64),
  installerSha256: hash,
  provenanceDocumentSha256: '3'.repeat(64),
  bytes: 100,
} as const;
const valid = {
  schemaVersion: 3,
  installer: expected.installer,
  architecture: 'x64',
  sourceCommit: expected.sourceCommit,
  sourceTree: expected.sourceTree,
  sourceTreeSha256: expected.sourceTreeSha256,
  installerProvenanceSha256: hash,
  provenanceDocumentSha256: expected.provenanceDocumentSha256,
  installerSha256Before: hash,
  installerSha256After: hash,
  bytes: 100,
  outerPeSubsystem: 'windows-gui',
  outerPeSubsystemValue: 2,
  setupWindow: { processId: 10, className: '#32770' },
  installerRoleExits: {
    'medium-controller': { pid: 10, exitCode: 0 },
    'elevated-worker': { pid: 11, exitCode: 0 },
  },
  processes: [
    { pid: 10, role: 'medium-controller', consoleWindow: false },
    { pid: 11, role: 'elevated-worker', consoleWindow: false },
  ],
  authenticatedPipe: {
    oneShot: true,
    controllerPid: 10,
    workerPid: 11,
    clientProcessIdVerified: true,
    serverProcessIdVerified: true,
    sameImageSha256Verified: true,
    challengeProofVerified: true,
  },
  packageManifest: {
    magic: 'TQPKG2',
    canonical: true,
    fullTreeVerified: true,
    architecture: 'x64',
  },
  powershellProcessStarts: 0,
  interpreterProcessStarts: 0,
  successfulDefaultLifecycle: true,
  forcedCleanup: false,
  authoritativeZeroResidue: true,
  activeProcessesAfterTeardown: [],
  residueAfterTeardown: [],
  passed: true,
};

describe('native Windows setup UI evidence', () => {
  it('requires medium controller, authenticated elevated worker, and TQPKG2 evidence', () => {
    expect(validateWindowsInstallerUiEvidence(valid, expected)).toBe(valid);
  });
  it.each([
    ['sameImageSha256Verified', false],
    ['challengeProofVerified', false],
    ['clientProcessIdVerified', false],
  ])('rejects missing pipe proof %s', (field, value) => {
    expect(() =>
      validateWindowsInstallerUiEvidence(
        { ...valid, authenticatedPipe: { ...valid.authenticatedPipe, [field]: value } },
        expected,
      ),
    ).toThrow();
  });
  it('requires authoritative zero residue', () => {
    expect(() =>
      validateWindowsInstallerUiEvidence(
        { ...valid, residueAfterTeardown: ['ProgramData/stale'] },
        expected,
      ),
    ).toThrow();
  });
});
