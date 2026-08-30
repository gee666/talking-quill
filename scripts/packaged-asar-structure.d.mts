export interface PackageTarget {
  readonly platform: 'win' | 'mac';
  readonly architecture: 'x64' | 'arm64';
}

export declare function verifyPackagedAsarStructure(context: {
  readonly appOutDir: string;
  readonly electronPlatformName: 'win32' | 'darwin' | string;
  readonly arch: number;
  readonly packager: { readonly appInfo: { readonly productFilename: string } };
}): Promise<void>;

export declare function verifyTargetNativeOnnxArchitecture(
  archive: string,
  target: PackageTarget,
): Promise<void>;
