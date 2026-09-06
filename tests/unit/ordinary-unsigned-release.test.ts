import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { validateStagedRelease } from '../../scripts/assemble-ordinary-unsigned-release.mjs';

const expected = {
  version: '0.0.73',
  architecture: 'arm64',
  sourceCommit: 'a'.repeat(40),
  sourceTree: 'b'.repeat(40),
  installer: 'Talking-Quill-0.0.73-win-arm64-setup.exe',
  bytes: 123,
  sha256: 'c'.repeat(64),
};
const release = {
  ...expected,
  schemaVersion: 1,
  result: 'passed',
  packageMode: 'fresh',
  variant: 'canonical',
  tqpkg2TreeSha256: 'd'.repeat(64),
};
describe('ordinary unsigned assembly', () => {
  it('accepts the staged RELEASE.json contract without requiring a signed publication envelope', () => {
    expect(() => validateStagedRelease(release, expected)).not.toThrow();
    for (const [key, value] of Object.entries(expected)) {
      expect(() =>
        validateStagedRelease({ ...release, [key]: `${String(value)}-wrong` }, expected),
      ).toThrow();
    }
    for (const key of ['result', 'packageMode', 'variant', 'tqpkg2TreeSha256']) {
      expect(() => validateStagedRelease({ ...release, [key]: 'wrong' }, expected)).toThrow();
    }
  });
  it('consumes exactly the four staged files and verifies native evidence and source provenance', () => {
    const script = readFileSync('scripts/assemble-ordinary-unsigned-release.mjs', 'utf8');
    expect(script).toContain("[installer, name, 'RELEASE.json', 'THIRD_PARTY_NOTICES.txt']");
    expect(script).toContain('await verifyHostedRuntimeEvidence');
    expect(script).toContain('observed.workflowRunId !== process.env.GITHUB_RUN_ID');
    expect(script).toContain('value.sourceTreeSha256 !== sourceTreeSha256');
    expect(script).toContain('value.sourceCommit !== identity.sourceCommit');
    expect(script).toContain('final[0].sha256 !== hash(bytes)');
    expect(script).not.toContain('publication-manifest');
    expect(script).not.toContain('privateKey');
  });
});
