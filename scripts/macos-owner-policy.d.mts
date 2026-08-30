export interface ReleasePolicyInput {
  readonly arch: 'x64' | 'arm64';
  readonly buildDigest: string;
  readonly gatewaySha256: string;
  readonly ownerSha256: string;
  readonly gatewayRequirement: string;
  readonly ownerRequirement: string;
  readonly predecessor?: null | {
    readonly releaseBuildDigest: string;
    readonly gatewaySha256: string;
    readonly ownerSha256: string;
  };
}
export function encodeReleasePolicy(input: ReleasePolicyInput): Buffer;
export interface ReleaseBuildIdentityInput {
  readonly packageVersion: string;
  readonly arch: 'x64' | 'arm64';
  readonly signingMode: 'adhoc' | 'self-signed';
  readonly cmsCertificateSha256: string;
  readonly codeCertificateSha256?: string;
  readonly codeCertificateSha1?: string;
  readonly gatewaySha256: string;
  readonly gatewayIdentifier: string;
  readonly gatewayCdhash: string;
  readonly gatewayRequirement: string;
  readonly ownerSha256: string;
  readonly ownerIdentifier: string;
  readonly ownerCdhash: string;
  readonly ownerRequirement: string;
  readonly bridgeSha256: string;
  readonly bridgeIdentifier: string;
  readonly bridgeCdhash: string;
  readonly bridgeRequirement: string;
  readonly predecessor?: ReleasePolicyInput['predecessor'];
}
export function deriveReleaseBuildDigest(input: ReleaseBuildIdentityInput): string;
export function verifyInstalledPolicyIdentities(
  wireText: string,
  input: {
    readonly gateway: string;
    readonly owner: string;
    readonly mode: 'adhoc' | 'self-signed';
  },
): Readonly<Record<string, string>>;
