export interface WindowsPromotionEvidenceOptions {
  readonly directory: string;
  readonly repository: string;
  readonly workflowRunId: string;
  readonly publicKeyPath: string;
  readonly rebootRunIds?: Readonly<{ x64: string; arm64: string }>;
}

export interface CreateWindowsPromotionEvidenceOptions extends WindowsPromotionEvidenceOptions {
  readonly output: string;
  readonly privateKeyPkcs8Base64: string;
  readonly updatePublicKeyPath: string;
  readonly rebootRunIds: Readonly<{ x64: string; arm64: string }>;
}

export interface VerifyWindowsPromotionEvidenceOptions extends WindowsPromotionEvidenceOptions {
  readonly path: string;
}

export interface WindowsPromotionEvidenceEnvelope {
  readonly payload: {
    readonly schemaVersion: 3;
    readonly promotionClass: 'protected-release-acceptance';
    readonly releasePolicy: {
      readonly version: '0.0.69';
      readonly mode: 'fresh-trust-root';
      readonly trustRootVersion: '0.0.69';
      readonly localMigration: {
        readonly sourceVersion: '0.0.67';
        readonly provenance: 'local-non-public';
        readonly mode: 'local-uninstall-preserve-fresh';
      };
    };
    readonly repository: string;
    readonly workflowRunId: string;
    readonly sourceCommit: string;
    readonly sourceTree: string;
    readonly promotionKeySha256: string;
    readonly rebootRunIds: Readonly<{ x64: string; arm64: string }>;
    readonly records: readonly unknown[];
  };
  readonly signature: {
    readonly scheme: 'p256-sha256-p1363-v1';
    readonly keyId: string;
    readonly value: string;
  };
}

export function createWindowsPromotionEvidence(
  options: CreateWindowsPromotionEvidenceOptions,
): Promise<WindowsPromotionEvidenceEnvelope>;
export function verifyWindowsPromotionEvidence(
  options: VerifyWindowsPromotionEvidenceOptions,
): Promise<WindowsPromotionEvidenceEnvelope>;
