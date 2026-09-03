export function sanitizedBuildEnvironment(
  environment?: Record<string, string | undefined>,
): Record<string, string>;
export function validateCanonicalRelease(options: {
  readonly descriptorPath: string;
  readonly descriptorSha256: string;
  readonly provenancePath: string;
  readonly provenanceSha256: string;
  readonly sourceRoot: string;
}): Promise<Readonly<Record<string, unknown>>>;
export function buildInstalledAcceptanceKit(
  options: {
    readonly descriptorPath: string;
    readonly descriptorSha256: string;
    readonly provenancePath?: string;
    readonly provenanceSha256?: string;
    readonly sourceRoot: string;
    readonly configPath: string;
    readonly requestPrivateKeyPath: string;
    readonly signerPath: string;
    readonly signerSha256: string;
    readonly outputRoot?: string;
    readonly bundlePath?: string;
  },
  dependencies?: {
    readonly validateCanonicalRelease?: (...arguments_: any[]) => any;
    readonly signAcceptancePayload?: (...arguments_: any[]) => any;
    readonly createInstalledAcceptancePlan?: (...arguments_: any[]) => any;
    readonly validateAcceptanceRunSequence?: (...arguments_: any[]) => any;
    readonly verifyAcceptancePreflight?: (...arguments_: any[]) => any;
    readonly protectNativeExecutionDirectory?: (path: string) => void;
  },
): Promise<Readonly<Record<string, unknown>>>;
