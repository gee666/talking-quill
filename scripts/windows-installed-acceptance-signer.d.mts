export interface NativeSignerResult {
  readonly signatureBase64url: string;
  readonly publicKeySpkiBase64url: string;
}
export function signAcceptancePayload(options: {
  readonly signerPath: string;
  readonly privateKeyPath: string;
  readonly signerSha256: string;
  readonly signerSourceCommit?: string;
  readonly signerSourceTree?: string;
  readonly payloadBytes: Buffer;
  readonly spawnProcess?: (...arguments_: any[]) => any;
}): NativeSignerResult;
