export interface PublicationObject {
  readonly sha256: string;
  readonly bytes: number;
  readonly objectName: string;
}

export interface PublicationAsset {
  readonly name: string;
  readonly objectSha256: string;
}

export interface PublicationPayload {
  readonly schemaVersion: 1;
  readonly repository: string;
  readonly tag: string;
  readonly sequence: number;
  readonly workflowRunId: string;
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly objects: readonly PublicationObject[];
  readonly assets: readonly PublicationAsset[];
  readonly promotionEvidenceSha256: string;
  readonly releaseManifestSha256: string;
}

export interface PublicationEnvelope {
  readonly payload: PublicationPayload;
  readonly signature: {
    readonly scheme: 'p256-sha256-p1363-v1';
    readonly keyId: string;
    readonly value: string;
  };
}

export interface PublicationManifestOptions {
  readonly directory: string;
  readonly repository: string;
  readonly tag: string;
  readonly sequence: number | string;
  readonly workflowRunId: string;
  readonly sourceCommit: string;
  readonly sourceTree: string;
  readonly publicKeyPath: string;
}

export function createPublicationManifest(
  options: PublicationManifestOptions & {
    readonly output: string;
    readonly privateKeyPkcs8Base64: string;
  },
): Promise<PublicationEnvelope>;

export function verifyPublicationEnvelope(options: {
  readonly envelope: PublicationEnvelope;
  readonly repository: string;
  readonly tag: string;
  readonly publicKeyPath: string;
}): Promise<PublicationEnvelope>;

export function verifyPublicationManifest(
  options: PublicationManifestOptions & { readonly path: string },
): Promise<PublicationEnvelope>;
