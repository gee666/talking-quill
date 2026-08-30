export interface RegularAsarFile {
  readonly entry: string;
  readonly bytes: Buffer;
  readonly metadata: {
    readonly size: number;
    readonly unpacked?: boolean;
    readonly offset?: string;
  };
}

export declare function extractRegularAsarFiles(
  archivePath: string,
  entries: readonly string[],
  label?: string,
): Generator<RegularAsarFile, void, undefined>;
