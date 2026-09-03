export interface NativeSignerResult {
  readonly signatureBase64url: string;
  readonly publicKeySpkiBase64url: string;
}
export function signAcceptancePayload(options: {
  readonly signerPath: string;
  readonly privateKeyPath: string;
  readonly signerSha256: string;
  readonly signerBytes: number;
  readonly signerSourceCommit?: string;
  readonly signerSourceTree?: string;
  readonly brokerPath?: string;
  readonly brokerSha256: string;
  readonly brokerBytes: number;
  readonly bootstrapIdentity: { readonly path: string; readonly sha256: string; readonly bytes: number };
  readonly payloadBytes: Buffer;
  readonly launchProcess?: (...arguments_: any[]) => any;
}): NativeSignerResult;
