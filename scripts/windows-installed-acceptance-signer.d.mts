export function signAcceptanceInput(input: {
  readonly operation: 'acceptance-envelope';
  readonly privateKeyPkcs8Base64: string;
  readonly payload: unknown;
}): Readonly<{
  encoded: string;
  publicKeySpkiBase64url: string;
}>;
export function signAcceptanceInput(input: {
  readonly operation: 'windows-update';
  readonly privateKeyPkcs8Base64: string;
  readonly packageSha256: string;
  readonly packageLayoutDigest: string;
}): Readonly<{
  scheme: 'p256-sha256-v1';
  signature: string;
  verificationKeySha256: string;
}>;
