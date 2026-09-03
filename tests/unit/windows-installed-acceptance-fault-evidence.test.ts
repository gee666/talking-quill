import { createHash, generateKeyPairSync, sign } from 'node:crypto';
import { describe, expect, it } from 'vitest';
import {
  encodeSignedFaultEvidence,
  faultEvidenceGenesis,
  verifyFaultEvidenceChain,
} from '../../scripts/windows-installed-acceptance-fault-evidence.mjs';
import { ACCEPTANCE_FAULT_PHASES } from '../../scripts/windows-installed-acceptance.mjs';
import { canonicalAcceptanceJson } from '../../scripts/windows-installed-acceptance-probe.mjs';

const hex = (value: string) => createHash('sha256').update(value).digest('hex');
const hashBytes = (value: Buffer) => createHash('sha256').update(value).digest('hex');

describe('installed-acceptance fault validation evidence', () => {
  it('verifies the dedicated signature, exact package binding, and complete hash chain', () => {
    const keys = generateKeyPairSync('ec', { namedCurve: 'prime256v1' });
    const publicKeySpkiBase64url = keys.publicKey
      .export({ format: 'der', type: 'spki' })
      .toString('base64url');
    const buildId = hex('build');
    const candidateSha256 = hex('candidate');
    const candidateLayoutDigest = hex('layout');
    let previous = faultEvidenceGenesis(buildId, candidateSha256);
    const records = ACCEPTANCE_FAULT_PHASES.map((faultPhase, sequence) => {
      const artifact = {
        installer: { sha256: hex(`fault-${faultPhase}`) },
        packageManifest: { treeSha256: hex(`tree-${faultPhase}`) },
      };
      const payload = {
        schemaVersion: 1,
        purpose: 'talking-quill/installed-acceptance-fault-validation',
        architecture: 'x64',
        buildId,
        sourceCommit: '11'.repeat(20),
        sourceTree: '22'.repeat(20),
        sequence,
        faultPhase,
        previousEnvelopeSha256: previous,
        candidatePackageSha256: candidateSha256,
        candidatePackageLayoutDigest: candidateLayoutDigest,
        faultPackageSha256: artifact.installer.sha256,
        faultPackageTreeSha256: artifact.packageManifest.treeSha256,
        validatorSha256: hex('validator'),
        namespaceIdSha256: hex(`namespace-${String(sequence)}`),
        machineIdentitySha256: hex('machine'),
        sessionIdentitySha256: hex('session'),
        productionStateBeforeSha256: hex('production'),
        productionStateAfterSha256: hex('production'),
        isolatedStateBeforeSha256: hex(`before-${String(sequence)}`),
        isolatedStateAfterSha256: hex(`after-${String(sequence)}`),
        faultExitCode: 197,
        recoveryExitCode: 0,
        failurePointObserved: faultPhase,
        failureInjected: true,
        recoveryCompleted: true,
        zeroResidue: true,
        productionStateUnchanged: true,
        mixedAuthorityAbsent: true,
      };
      const signatureBase64url = sign('sha256', Buffer.from(canonicalAcceptanceJson(payload)), {
        key: keys.privateKey,
        dsaEncoding: 'ieee-p1363',
      }).toString('base64url');
      const bytes = encodeSignedFaultEvidence(payload, { signatureBase64url });
      previous = hashBytes(bytes);
      return { bytes, artifact };
    });
    const verified = verifyFaultEvidenceChain(records, {
      buildId,
      candidateSha256,
      candidateLayoutDigest,
      architecture: 'x64',
      sourceCommit: '11'.repeat(20),
      sourceTree: '22'.repeat(20),
      publicKeySpkiBase64url,
      chainHeadSha256: previous,
    });
    expect(verified.chainHeadSha256).toBe(previous);
    const forged = records.map((record) => ({ ...record }));
    const target = forged[3];
    if (target === undefined) throw new Error('Fault evidence fixture is incomplete');
    target.bytes = Buffer.from(target.bytes);
    target.bytes[20] = (target.bytes[20] ?? 0) ^ 1;
    expect(() =>
      verifyFaultEvidenceChain(forged, {
        buildId,
        candidateSha256,
        candidateLayoutDigest,
        architecture: 'x64',
        sourceCommit: '11'.repeat(20),
        sourceTree: '22'.repeat(20),
        publicKeySpkiBase64url,
      }),
    ).toThrow();
  });
});
