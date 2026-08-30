export interface ElectronBuilderConfig {
  readonly files?: unknown;
  readonly asarUnpack?: unknown;
  readonly win?: unknown;
  readonly mac?: unknown;
  readonly [key: string]: unknown;
}

export function loadMergedElectronBuilderConfig(configPath: string): Promise<ElectronBuilderConfig>;
export function validateElectronBuilderConfigFile(configPath: string): Promise<void>;
export function validatePackageElectronBuilderConfigs(): Promise<void>;
