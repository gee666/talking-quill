export type NativeArchitecture = 'x86' | 'x64' | 'arm64';
export type NativeFormat = 'pe' | 'mach-o-thin' | 'mach-o-fat';
export interface NativeArchitectureInspection {
  readonly format: NativeFormat;
  readonly architectures: readonly NativeArchitecture[];
}
export interface NativeTreeEntry extends NativeArchitectureInspection {
  readonly path: string;
}

export function readNativeArchitectures(path: string): Promise<NativeArchitectureInspection | null>;
export function parseNativeArchitectures(
  bytes: Buffer,
  path: string,
): NativeArchitectureInspection | null;
export function inspectNativeTree(
  root: string,
  options: {
    readonly platform: 'win' | 'mac';
    readonly architecture: Exclude<NativeArchitecture, 'x86'>;
    readonly exceptions?: Readonly<Record<string, NativeArchitecture>>;
  },
): Promise<readonly NativeTreeEntry[]>;
