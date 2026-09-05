export interface WindowsReleaseValidation {
  readonly schemaVersion: 1;
  readonly scope: 'automated-fresh-release';
  readonly realReboot: 'not-collected';
  readonly protectedInstalledAcceptance: 'not-collected';
  readonly evidence: readonly { readonly name: string; readonly sha256: string }[];
}
export function windowsReleaseValidation(
  directory: string,
  verify?: boolean,
): Promise<WindowsReleaseValidation>;
