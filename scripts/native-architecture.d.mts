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
export function readNativeArchitecture(path: string, mac: boolean): Promise<NativeArchitecture>;
export function parseNativeArchitectures(
  bytes: Buffer,
  path: string,
): NativeArchitectureInspection | null;
export function parseNativeArchitecture(
  bytes: Buffer,
  path: string,
  mac: boolean,
): NativeArchitecture;
export function inspectNativeTree(
  root: string,
  expectedArchitecture: Exclude<NativeArchitecture, 'x86'>,
  exceptions?: Readonly<Record<string, NativeArchitecture>>,
): Promise<readonly NativeTreeEntry[]>;
