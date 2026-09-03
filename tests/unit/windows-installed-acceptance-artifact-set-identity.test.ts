import { describe, expect, it } from 'vitest';
import { createProducerArtifactSetIdentity } from '../../scripts/windows-installed-acceptance-artifact-set-identity.mjs';
import { ACCEPTANCE_FAULT_PHASES } from '../../scripts/windows-installed-acceptance-schedule.mjs';

const hash = (value: string) => value.repeat(64);
function fixture() {
  return {
    schemaVersion: 1,
    sourceCommit: 'a'.repeat(40),
    sourceTree: 'b'.repeat(40),
    buildId: hash('1'),
    candidate: {
      electronSha256: hash('2'),
      appAsarSha256: hash('3'),
      metadataSha256: hash('4'),
      releaseIdentitySha256: hash('5'),
      packageLayoutDigest: hash('6'),
      gatewaySha256: hash('7'),
      ownerSha256: hash('8'),
    },
    update: { installerSha256: hash('9'), packageLayoutDigest: hash('a') },
    repair: { installerSha256: hash('b'), packageLayoutDigest: hash('c') },
    faults: ACCEPTANCE_FAULT_PHASES.map((phase, index) => ({
      phase,
      installerSha256: index.toString(16).repeat(64),
      packageLayoutDigest: (15 - index).toString(16).repeat(64),
    })),
    native: {
      bootstrapSha256: hash('d'),
      brokerSha256: hash('e'),
      signerSha256: hash('f'),
      senderSha256: hash('0'),
    },
    embeddedManifest: { sha256: hash('1'), validationKeySha256: hash('2') },
    faultChainHeadSha256: hash('3'),
    bundlePayloadInventory: [
      { path: 'a.bin', bytes: 1, sha256: hash('4') },
      { path: 'z.bin', bytes: 2, sha256: hash('5') },
    ],
  };
}

describe('producer artifact-set identity', () => {
  it('has a stable canonical digest and covers the complete payload inventory', () => {
    const input = fixture();
    expect(createProducerArtifactSetIdentity(input)).toBe(
      '818458665848b333c0b7b02c8743151527b2be9af76f076c5d49506cde876e88',
    );
    expect(
      createProducerArtifactSetIdentity({
        ...input,
        bundlePayloadInventory: [
          input.bundlePayloadInventory[0],
          { ...input.bundlePayloadInventory[1], sha256: hash('6') },
        ],
      }),
    ).not.toBe(createProducerArtifactSetIdentity(input));
  });

  it('rejects reordered faults and cyclic producer/manifest inventory entries', () => {
    const input = fixture();
    expect(() =>
      createProducerArtifactSetIdentity({ ...input, faults: [...input.faults].reverse() }),
    ).toThrow('identity payload is invalid');
    for (const path of ['producer-result.json', 'bundle-manifest.json']) {
      expect(() =>
        createProducerArtifactSetIdentity({
          ...input,
          bundlePayloadInventory: [{ path, bytes: 1, sha256: hash('4') }],
        }),
      ).toThrow('identity payload is invalid');
    }
  });
});
