export function faultEvidenceGenesis(buildId: string, candidateSha256: string): string;
export function encodeSignedFaultEvidence(
  payload: Readonly<Record<string, unknown>>,
  signed: Readonly<{ signatureBase64url: string }>,
): Buffer;
export function verifyFaultEvidenceChain(
  records: readonly Readonly<{ bytes: Buffer; artifact: any }>[],
  expectation: Readonly<Record<string, any>>,
): Readonly<{ chainHeadSha256: string; envelopeSha256: readonly string[] }>;
