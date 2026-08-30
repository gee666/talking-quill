import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import {
  MACOS_R11_CHECKPOINTS,
  canonicalJson,
  sealMacosR11Evidence,
  validateMacosR11Evidence as validateMacosR11EvidenceRaw,
} from '../../scripts/macos-r11-evidence.mjs';
import { authenticateMacosR11Provenance } from '../../scripts/macos-r11-provenance.mjs';

const digest = (character: string) => character.repeat(64);
const sourceCommit = 'a'.repeat(40);
const sessionBindingSha256 = digest('e');
const { privateKey: operatorPrivateKey, publicKey: operatorPublicKey } =
  generateKeyPairSync('ed25519');
const operatorPublicDer = operatorPublicKey.export({ format: 'der', type: 'spki' });
const operatorPublicKeySha256 = createHash('sha256').update(operatorPublicDer).digest('hex');

interface FixtureCheckpoint {
  id: string;
  method: string;
  challengeSha256: string;
  observedAt: number;
  result: string;
  attestation: ReturnType<typeof operatorAttestation> | null;
}
interface FixtureEvidence {
  schemaVersion: number;
  kind: string;
  platform: string;
  arch: string;
  sourceCommit: string;
  candidateTag: string;
  sessionBindingSha256: string;
  releaseRun: { id: string; attempt: string };
  lifecycleRun: { id: string; attempt: string };
  runner: { os: string; arch: string; name: string };
  signing: {
    mode: string;
    candidateRequirement: string;
    baselineRequirement: string;
    candidateLeafSha256: string | null;
    baselineLeafSha256: string | null;
    candidateCdHash: string;
    baselineCdHash: string;
  };
  artifacts: {
    candidateDmg: ReturnType<typeof artifact>;
    candidateZip: ReturnType<typeof artifact>;
    baselineZip: ReturnType<typeof artifact>;
  };
  installed: Record<string, string>;
  checkpoints: FixtureCheckpoint[];
  result: string;
  evidenceSha256?: string;
  [key: string]: unknown;
}

function evidence(overrides: Record<string, unknown> = {}): FixtureEvidence {
  const body = {
    schemaVersion: 1,
    kind: 'macos-r11-installed-lifecycle',
    platform: 'mac',
    arch: 'arm64',
    sourceCommit,
    candidateTag: 'v1.2.3',
    sessionBindingSha256,
    releaseRun: { id: '42', attempt: '2' },
    lifecycleRun: { id: '84', attempt: '1' },
    runner: { os: 'macOS', arch: 'ARM64', name: 'permissioned-arm64' },
    signing: {
      mode: 'self-signed',
      candidateRequirement: 'identifier com.talkingquill.app and certificate leaf = H"abcd"',
      baselineRequirement: 'identifier com.talkingquill.app and certificate leaf = H"abcd"',
      candidateLeafSha256: digest('1'),
      baselineLeafSha256: digest('1'),
      candidateCdHash: '2'.repeat(40),
      baselineCdHash: '3'.repeat(40),
    },
    artifacts: {
      candidateDmg: artifact('Talking-Quill-1.2.3-mac-arm64.dmg', '4'),
      candidateZip: artifact('Talking-Quill-1.2.3-mac-arm64.zip', '5'),
      baselineZip: artifact('Talking-Quill-1.2.2-mac-arm64.zip', '6'),
    },
    predecessor: {
      platform: 'mac',
      architecture: 'arm64',
      version: '1.2.2',
      runId: '41',
      runAttempt: '1',
      headSha: '9'.repeat(40),
      artifactName: 'Talking-Quill-1.2.2-mac-arm64.zip',
      releaseBuildDigest: digest('b'),
      gatewaySha256: digest('b'),
      ownerSha256: digest('c'),
      artifactSha256: digest('6'),
    },
    installed: {
      dmgAppTreeSha256: digest('7'),
      zipAppTreeSha256: digest('7'),
      updatedAppTreeSha256: digest('7'),
      rollbackAppTreeSha256: digest('8'),
      candidateGatewaySha256: digest('9'),
      candidateOwnerSha256: digest('a'),
      baselineGatewaySha256: digest('b'),
      baselineOwnerSha256: digest('c'),
      installationIdSha256: digest('d'),
    },
    checkpoints: MACOS_R11_CHECKPOINTS.map((id, index) => {
      const challengeSha256 = digest(((index % 9) + 1).toString());
      const observedAt = 1_700_000_000_000 + index;
      const manual = manualIds.has(id);
      return {
        id,
        method: manual ? 'staffed-manual' : 'automated',
        challengeSha256,
        observedAt,
        result: 'passed',
        attestation: manual ? operatorAttestation(id, challengeSha256, observedAt) : null,
      };
    }),
    result: 'passed',
    ...overrides,
  };
  return sealMacosR11Evidence(body);
}

const manualIds = new Set([
  'local-install-anyway',
  'keychain-owner-allow',
  'tcc-grant-capture',
  'tcc-revoke-fail-closed',
  'tcc-regrant-recovery',
  'baseline-tcc-grant-capture',
  'candidate-tcc-post-update-recovery',
  'persisted-identity-continuity',
  'drag-to-trash-cleanup',
  'controlled-uninstall-cleanup',
  'physical-shortcut-held',
  'physical-shortcut-replay',
  'physical-option-command-replay',
  'physical-paste',
]);

function operatorAttestation(id: string, challengeSha256: string, observedAt: number) {
  const statement = {
    id,
    challengeSha256,
    sessionBindingSha256,
    result: 'passed',
    observedAt,
    operator: 'staffed-tester',
    host: 'permissioned-mac',
  };
  const payload = Buffer.from(canonicalJson(statement));
  return {
    algorithm: 'ed25519',
    publicKeySpkiBase64: operatorPublicDer.toString('base64'),
    publicKeySha256: operatorPublicKeySha256,
    payloadBase64: payload.toString('base64'),
    signatureBase64: sign(null, payload, operatorPrivateKey).toString('base64'),
  };
}

function artifact(name: string, character: string) {
  return { name, bytes: 123, sha256Before: digest(character), sha256After: digest(character) };
}

function reseal(value: FixtureEvidence): FixtureEvidence {
  const copy = structuredClone(value);
  delete copy.evidenceSha256;
  return sealMacosR11Evidence(copy);
}
function validateMacosR11Evidence(
  value: FixtureEvidence,
  expected: Partial<Parameters<typeof validateMacosR11EvidenceRaw>[1]> = {},
) {
  return validateMacosR11EvidenceRaw(value, {
    ...expected,
    operatorPublicKeySha256: expected.operatorPublicKeySha256 ?? operatorPublicKeySha256,
    sessionBindingSha256: expected.sessionBindingSha256 ?? sessionBindingSha256,
    evidenceSha256: expected.evidenceSha256 ?? value.evidenceSha256 ?? '',
  });
}

describe('macOS R11 exact-artifact evidence semantics', () => {
  it('accepts a sealed native ARM64 record and binds expected release/artifact identity', () => {
    const value = evidence();
    expect(
      validateMacosR11Evidence(value, {
        arch: 'arm64',
        sourceCommit,
        candidateTag: 'v1.2.3',
        operatorPublicKeySha256,
        artifactSha256: {
          candidateDmg: digest('4'),
          candidateZip: digest('5'),
          baselineZip: digest('6'),
        },
      }),
    ).toBe(value);
  });

  it('rejects changed artifact bytes, cross-architecture runners, and DMG/ZIP tree substitution', () => {
    const changed = evidence();
    changed.artifacts.candidateZip.sha256After = digest('0');
    expect(() => validateMacosR11Evidence(reseal(changed))).toThrow('Artifact bytes changed');

    const crossTarget = evidence({ runner: { os: 'macOS', arch: 'X64', name: 'wrong' } });
    expect(() => validateMacosR11Evidence(crossTarget)).toThrow('not native');

    const substituted = evidence();
    substituted.installed.updatedAppTreeSha256 = digest('0');
    expect(() => validateMacosR11Evidence(reseal(substituted))).toThrow('application trees differ');
  });

  it('rejects missing, duplicate, failed, or falsely automated physical/manual checkpoints', () => {
    const missing = evidence();
    missing.checkpoints.pop();
    expect(() => validateMacosR11Evidence(reseal(missing))).toThrow('Incomplete');

    const duplicate = evidence();
    const firstCheckpoint = duplicate.checkpoints[0];
    if (firstCheckpoint === undefined) throw new Error('Missing fixture checkpoint');
    duplicate.checkpoints[1] = { ...firstCheckpoint };
    expect(() => validateMacosR11Evidence(reseal(duplicate))).toThrow('duplicate');

    const failed = evidence();
    const failedCheckpoint = failed.checkpoints[0];
    if (failedCheckpoint === undefined) throw new Error('Missing fixture checkpoint');
    failedCheckpoint.result = 'failed';
    expect(() => validateMacosR11Evidence(reseal(failed))).toThrow('did not pass');

    const automatedTcc = evidence();
    const tccCheckpoint = automatedTcc.checkpoints.find(
      (entry) => entry.id === 'tcc-grant-capture',
    );
    if (tccCheckpoint === undefined) throw new Error('Missing fixture checkpoint');
    tccCheckpoint.method = 'automated';
    tccCheckpoint.attestation = null;
    expect(() => validateMacosR11Evidence(reseal(automatedTcc))).toThrow('was automated');

    const automatedRemoval = evidence();
    const removal = automatedRemoval.checkpoints.find(
      (entry) => entry.id === 'drag-to-trash-cleanup',
    );
    if (removal === undefined) throw new Error('Missing removal checkpoint');
    removal.method = 'automated';
    removal.attestation = null;
    expect(() => validateMacosR11Evidence(reseal(automatedRemoval))).toThrow('was automated');
  });

  it('enforces persistent self-signed identity and honest ad-hoc identity semantics', () => {
    const changedLeaf = evidence();
    changedLeaf.signing.baselineLeafSha256 = digest('e');
    expect(() => validateMacosR11Evidence(reseal(changedLeaf))).toThrow('exact local certificate');

    const adhoc = evidence();
    adhoc.signing = {
      ...adhoc.signing,
      mode: 'adhoc',
      candidateRequirement: 'identifier com.talkingquill.app and cdhash H"1111"',
      baselineRequirement: 'identifier com.talkingquill.app and cdhash H"2222"',
      candidateLeafSha256: null,
      baselineLeafSha256: null,
    };
    expect(() => validateMacosR11Evidence(reseal(adhoc))).not.toThrow();

    adhoc.signing.candidateLeafSha256 = digest('f');
    expect(() => validateMacosR11Evidence(reseal(adhoc))).toThrow('must not claim a certificate');
  });

  it('requires external trust inputs and verifies operator signatures/session binding', () => {
    expect(() =>
      validateMacosR11EvidenceRaw(evidence(), {
        operatorPublicKeySha256: '',
        sessionBindingSha256,
        evidenceSha256: digest('0'),
      }),
    ).toThrow('Trusted operator public key pin is required');
    expect(() =>
      validateMacosR11EvidenceRaw(evidence(), {
        operatorPublicKeySha256,
        sessionBindingSha256: digest('0'),
        evidenceSha256: digest('0'),
      }),
    ).toThrow('session binding mismatch');
    expect(() => validateMacosR11Evidence(evidence(), { evidenceSha256: digest('0') })).toThrow(
      'evidence seal mismatch',
    );

    const forged = evidence();
    const manual = forged.checkpoints.find((entry) => entry.method === 'staffed-manual');
    if (manual?.attestation === null || manual?.attestation === undefined) {
      throw new Error('Missing fixture attestation');
    }
    manual.attestation.signatureBase64 = Buffer.alloc(64).toString('base64');
    expect(() => validateMacosR11Evidence(reseal(forged), { operatorPublicKeySha256 })).toThrow(
      'Invalid operator attestation',
    );

    const wrongPin = evidence();
    expect(() =>
      validateMacosR11Evidence(wrongPin, { operatorPublicKeySha256: digest('0') }),
    ).toThrow('Unexpected operator attestation key');
  });

  it('accepts x64 only with an X64 runner and x64-targeted artifact names', () => {
    const value = evidence({
      arch: 'x64',
      runner: { os: 'macOS', arch: 'X64', name: 'permissioned-x64' },
      artifacts: {
        candidateDmg: artifact('Talking-Quill-1.2.3-mac-x64.dmg', '4'),
        candidateZip: artifact('Talking-Quill-1.2.3-mac-x64.zip', '5'),
        baselineZip: artifact('Talking-Quill-1.2.2-mac-x64.zip', '6'),
      },
      predecessor: {
        platform: 'mac',
        architecture: 'x64',
        version: '1.2.2',
        runId: '41',
        runAttempt: '1',
        headSha: '9'.repeat(40),
        artifactName: 'Talking-Quill-1.2.2-mac-x64.zip',
        releaseBuildDigest: digest('b'),
        gatewaySha256: digest('b'),
        ownerSha256: digest('c'),
        artifactSha256: digest('6'),
      },
    });
    expect(() => validateMacosR11Evidence(value, { arch: 'x64' })).not.toThrow();
  });

  it('rejects a lifecycle predecessor that differs from the release identity binding', () => {
    const wrongVersion = evidence();
    (wrongVersion.predecessor as { version: string }).version = '1.2.1';
    expect(() => validateMacosR11Evidence(reseal(wrongVersion))).toThrow('predecessor identity');

    const wrongHash = evidence();
    (wrongHash.predecessor as { artifactSha256: string }).artifactSha256 = digest('0');
    expect(() => validateMacosR11Evidence(reseal(wrongHash))).toThrow('predecessor identity');
  });

  it('detects any post-seal edit and rejects extra self-authorizing fields', () => {
    const edited = evidence();
    edited.result = 'failed';
    expect(() => validateMacosR11Evidence(edited)).toThrow('did not pass');

    const extra = { ...evidence(), approved: true };
    expect(() => validateMacosR11Evidence(extra)).toThrow('unexpected or missing fields');
  });

  it('keeps the macOS producer and installed-lifecycle consumer artifact names coherent', () => {
    const producer = readFileSync('.github/workflows/build-mac-local-owner.yml', 'utf8');
    const consumer = readFileSync('.github/workflows/macos-r11-installed-lifecycle.yml', 'utf8');
    for (const arch of ['x64', 'arm64']) {
      const name = `Talking-Quill-local-owner-mac-${arch}`;
      expect(producer).toContain(`name: ${name.replace(arch, '${{ matrix.arch }}')}`);
      expect(consumer).toContain(name);
    }
    expect(consumer).not.toContain('release-part-mac-');
  });

  it('authenticates immutable release workflow runs and architecture-specific artifacts', () => {
    const run = (id: number) => ({
      id,
      head_repository: { full_name: 'gee666/talking-quill' },
      path: '.github/workflows/build-mac-local-owner.yml',
      status: 'completed',
      conclusion: 'success',
      event: 'workflow_dispatch',
      head_branch: 'main',
      head_sha: id === 10 ? sourceCommit : 'b'.repeat(40),
      run_attempt: 1,
    });
    const artifacts = (id: number) => ({
      artifacts: [
        {
          id: id + 100,
          name: 'Talking-Quill-local-owner-mac-arm64',
          expired: false,
          workflow_run: { id },
        },
      ],
    });
    const value = authenticateMacosR11Provenance({
      arch: 'arm64',
      repository: 'gee666/talking-quill',
      repositoryData: { full_name: 'gee666/talking-quill', default_branch: 'main' },
      candidateRunId: '10',
      baselineRunId: '9',
      candidateRun: run(10),
      baselineRun: run(9),
      candidateArtifacts: artifacts(10),
      baselineArtifacts: artifacts(9),
      baselineZipSha256: digest('a'),
    });
    expect(value.candidate.artifact.name).toBe('Talking-Quill-local-owner-mac-arm64');

    expect(() =>
      authenticateMacosR11Provenance({
        arch: 'arm64',
        repository: 'gee666/talking-quill',
        repositoryData: { full_name: 'gee666/talking-quill', default_branch: 'main' },
        candidateRunId: '10',
        baselineRunId: '9',
        candidateRun: { ...run(10), conclusion: 'failure' },
        baselineRun: run(9),
        candidateArtifacts: artifacts(10),
        baselineArtifacts: artifacts(9),
        baselineZipSha256: digest('a'),
      }),
    ).toThrow('Unapproved candidate');
  });

  it('keeps the native harness runnable cross-host without pretending native execution', () => {
    const source = readFileSync('scripts/macos-r11-installed-lifecycle.mjs', 'utf8');
    for (const boundary of [
      '--print-checkpoints',
      "process.platform !== 'darwin'",
      'hdiutil',
      '--update-local-owner=',
      '--rollback-local-owner=',
      '--uninstall-local-owner',
      '--macos-owner-acl-denial-test',
      'held-key-finalizer-postponed',
      'drag-to-trash-cleanup',
    ]) {
      expect(source).toContain(boundary);
    }
  });
});
