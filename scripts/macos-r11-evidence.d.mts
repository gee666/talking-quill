export interface MacosR11Checkpoint {
  readonly id: string;
  readonly method: 'automated' | 'staffed-manual';
  readonly challengeSha256: string;
  readonly observedAt: number;
  readonly result: 'passed';
  readonly attestation: null | {
    readonly algorithm: 'ed25519';
    readonly publicKeySpkiBase64: string;
    readonly publicKeySha256: string;
    readonly payloadBase64: string;
    readonly signatureBase64: string;
  };
}

export interface MacosR11Evidence {
  readonly schemaVersion: 1;
  readonly kind: 'macos-r11-installed-lifecycle';
  readonly platform: 'mac';
  readonly arch: 'x64' | 'arm64';
  readonly sourceCommit: string;
  readonly candidateTag: string;
  readonly sessionBindingSha256: string;
  readonly predecessor: {
    readonly platform: 'mac';
    readonly architecture: 'x64' | 'arm64';
    readonly version: string;
    readonly runId: string;
    readonly runAttempt: string;
    readonly headSha: string;
    readonly artifactName: string;
    readonly releaseBuildDigest: string;
    readonly gatewaySha256: string;
    readonly ownerSha256: string;
    readonly artifactSha256: string;
  };
  readonly checkpoints: readonly MacosR11Checkpoint[];
  readonly result: 'passed';
  readonly evidenceSha256: string;
  readonly [key: string]: unknown;
}

export const MACOS_R11_SCHEMA_VERSION: 1;
export const MACOS_R11_KIND: 'macos-r11-installed-lifecycle';
export const MACOS_R11_CHECKPOINTS: readonly string[];
export function sha256File(path: string): string;
export function canonicalJson(value: unknown): string;
export function sealMacosR11Evidence<T extends object>(
  body: T,
): T & { readonly evidenceSha256: string };
export function validateMacosR11Evidence(
  value: unknown,
  expected: {
    readonly operatorPublicKeySha256: string;
    readonly sessionBindingSha256: string;
    readonly evidenceSha256: string;
    readonly arch?: 'x64' | 'arm64';
    readonly sourceCommit?: string;
    readonly candidateTag?: string;
    readonly artifactSha256?: Readonly<Record<string, string>>;
  },
): MacosR11Evidence;
export function writeMacosR11Evidence(
  path: string,
  body: object,
  expected: object,
): MacosR11Evidence;
