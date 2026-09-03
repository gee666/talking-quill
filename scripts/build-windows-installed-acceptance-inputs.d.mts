export function installedAcceptanceBuildEnvironment(
  environment?: Record<string, string | undefined>,
  additions?: Record<string, string | undefined>,
): Record<string, string>;

export interface WindowsInstalledAcceptanceProducerOptions {
  readonly architecture: 'x64';
  readonly descriptorPath: string;
  readonly descriptorSha256: string;
  readonly provenancePath: string;
  readonly provenanceSha256: string;
  readonly sourceRoot: string;
  readonly requestPrivateKeyPath: string;
  readonly signerPath: string;
  readonly signerSha256: string;
  readonly manifestPrivateKeyPath: string;
  readonly updatePrivateKeyPath: string;
  readonly validationPrivateKeyPath: string;
  readonly notBeforeMs: number | string;
  readonly expiresAtMs: number | string;
  readonly buildId?: string;
  readonly outputRoot?: string;
  readonly kitOutputRoot?: string;
  readonly bundlePath?: string;
  readonly assemble?: boolean;
  readonly heavyBuildDriverPath?: string;
}

export function buildWindowsInstalledAcceptanceInputs(
  options: WindowsInstalledAcceptanceProducerOptions,
  dependencies?: Readonly<Record<string, (...arguments_: any[]) => any>>,
): Promise<Readonly<Record<string, unknown>>>;
