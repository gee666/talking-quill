export interface NativeExecutableIdentity {
  readonly path: string;
  readonly sha256: string;
  readonly bytes: number;
}

export interface WindowsUpdateNativeSigner {
  readonly identities: {
    readonly signer: NativeExecutableIdentity;
    readonly broker: NativeExecutableIdentity;
    readonly bootstrap: NativeExecutableIdentity;
  };
  sign(payloadBytes: Buffer): {
    readonly publicKeySec1: Buffer;
    readonly signatureDer: Buffer;
  };
}

export function createWindowsUpdateNativeSigner(options: {
  readonly privateKeyPath: string;
  readonly signerPath: string;
  readonly brokerPath: string;
  readonly bootstrapPath: string;
  readonly signerSourceCommit?: string;
  readonly signerSourceTree?: string;
  readonly signerSha256?: string;
  readonly brokerSha256?: string;
  readonly bootstrapSha256?: string;
  readonly launchProcess?: (options: {
    readonly input: Buffer;
    readonly [key: string]: unknown;
  }) => {
    readonly status?: number | null;
    readonly signal?: NodeJS.Signals | null;
    readonly error?: Error;
    readonly stderr?: string;
    readonly stdout?: string;
  };
}): WindowsUpdateNativeSigner;
