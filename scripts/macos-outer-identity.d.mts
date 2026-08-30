export interface MacosOuterIdentity {
  readonly mode: 'certificate' | 'adhoc';
  readonly leafCertificateSha256: string | null;
  readonly identifier: string;
  readonly teamIdentifier: string | null;
  readonly designatedRequirement: string;
  readonly designatedRequirementSha256: string;
}
export function inspectMacosOuterIdentity(path: string): MacosOuterIdentity;
export function parseMacosCodesignIdentity(
  output: string,
  leafCertificateSha256?: string | null,
): MacosOuterIdentity;
