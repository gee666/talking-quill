export function sanitizedBuildEnvironment(
  environment?: Record<string, string | undefined>,
): Record<string, string>;
export function validateCanonicalRelease(options: {
  readonly descriptorPath: string;
  readonly descriptorSha256: string;
  readonly sourceRoot: string;
}): Promise<Readonly<Record<string, unknown>>>;
export function buildInstalledAcceptanceKit(options: {
  readonly descriptorPath: string;
  readonly descriptorSha256: string;
  readonly sourceRoot: string;
  readonly configPath: string;
  readonly requestPrivateKeyPath: string;
  readonly outputRoot?: string;
}): Promise<Readonly<Record<string, unknown>>>;
