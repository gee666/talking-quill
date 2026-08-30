export interface RegularAsarFile {
  readonly entry: string;
  readonly bytes: Buffer;
  readonly metadata: {
    readonly size: number;
    readonly unpacked?: boolean;
    readonly offset?: string;
  };
}

export interface AsarEntryInspectionOptions {
  readonly targetPlatform?: 'mac' | 'win';
  readonly targetArchitecture?: 'arm64' | 'x64';
}

export declare function extractRegularAsarFiles(
  archivePath: string,
  entries: readonly string[],
  label?: string,
  options?: AsarEntryInspectionOptions,
): Generator<RegularAsarFile, void, undefined>;
