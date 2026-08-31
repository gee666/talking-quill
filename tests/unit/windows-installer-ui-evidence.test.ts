import { describe, expect, it } from 'vitest';
import { validateWindowsInstallerUiEvidence } from '../../scripts/windows-installer-ui-evidence.mjs';

const hash = '1'.repeat(64);
const expected = {
  installer: 'Talking-Quill-0.0.69-win-x64.exe', architecture: 'x64',
  sourceCommit: 'a'.repeat(40), sourceTree: 'b'.repeat(40), sourceTreeSha256: '2'.repeat(64),
  installerSha256: hash, provenanceDocumentSha256: '3'.repeat(64), bytes: 100,
} as const;
const valid = {
  schemaVersion: 4, installer: expected.installer, architecture: 'x64',
  sourceCommit: expected.sourceCommit, sourceTree: expected.sourceTree,
  sourceTreeSha256: expected.sourceTreeSha256, installerProvenanceSha256: hash,
  provenanceDocumentSha256: expected.provenanceDocumentSha256,
  installerSha256Before: hash, installerSha256After: hash, bytes: 100,
  outerPeSubsystem: 'windows-gui', outerPeSubsystemValue: 2,
  setupWindow: { processId: 10, className: '#32770' },
  cancel: { commandSent: true, exitCode: 1223, workerStarted: false, pipeObserved: false },
  processStarts: [], interpreterProcessStarts: [], observerErrors: [], forcedCleanup: false,
  activeProcessesAfterTeardown: [], residueBefore: [], residueAfter: [], newResidueAfterCancel: [], passed: true,
};

describe('native Windows setup UI evidence', () => {
  it('requires a real pre-consent Cancel with no worker, pipe, interpreter, or mutation', () => {
    expect(validateWindowsInstallerUiEvidence(valid, expected)).toBe(valid);
  });
  it.each([
    ['commandSent', false], ['workerStarted', true], ['pipeObserved', true], ['exitCode', 0],
  ])('rejects invalid cancellation evidence %s', (field, value) => {
    expect(() => validateWindowsInstallerUiEvidence({ ...valid, cancel: { ...valid.cancel, [field]: value } }, expected)).toThrow();
  });
  it('rejects new residue and interpreter starts', () => {
    expect(() => validateWindowsInstallerUiEvidence({ ...valid, newResidueAfterCancel: ['ProgramData/stale'] }, expected)).toThrow();
    expect(() => validateWindowsInstallerUiEvidence({ ...valid, residueAfter: ['ProgramData/stale'], newResidueAfterCancel: [] }, expected)).toThrow();
    expect(() => validateWindowsInstallerUiEvidence({ ...valid, interpreterProcessStarts: ['powershell.exe'] }, expected)).toThrow();
  });
});
