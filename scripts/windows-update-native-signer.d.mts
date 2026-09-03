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
    readonly source: {
      readonly sourceCommit: string;
      readonly sourceTree: string;
    };
    readonly cargoLock: {
      readonly sha256: string;
      readonly blob: string;
    };
    readonly provenancePath: string;
  };
  sign(payloadBytes: Buffer): {
    readonly publicKeySec1: Buffer;
    readonly signatureDer: Buffer;
  };
}

export function createWindowsUpdateNativeSigner(options: {
  readonly privateKeyPath: string;
}): WindowsUpdateNativeSigner;
