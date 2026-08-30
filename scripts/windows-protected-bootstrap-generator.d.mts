export const protectedBootstrapSourcePath: string;
export const protectedBootstrapIncludePath: string;

export function canonicalizeProtectedBootstrapSource(source: Buffer | string): string;
export function renderProtectedBootstrapInclude(source: Buffer | string): string;
export function checkProtectedBootstrapInclude(options?: {
  readonly sourcePath?: string;
  readonly includePath?: string;
}): Promise<string>;
export function writeProtectedBootstrapInclude(options?: {
  readonly sourcePath?: string;
  readonly includePath?: string;
}): Promise<string>;
