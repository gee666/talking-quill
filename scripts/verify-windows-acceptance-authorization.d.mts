export function verifyWindowsAcceptanceAuthorization(input: {
  readonly authorizationBase64url: string;
  readonly publicKeySpkiBase64url: string;
  readonly bundleUrl: string;
  readonly bundleSha256: string;
  readonly architecture: 'x64' | 'arm64';
  readonly producerArtifactSetIdentity?: string;
  readonly sourceRevision?: string;
  readonly manifestPublicKeySpkiBase64url?: string;
  readonly nowMs: number;
}): Readonly<Record<string, unknown>>;
