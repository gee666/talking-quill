export interface NsisUninstallSources {
  readonly custom: string;
  readonly assisted: string;
  readonly uninstaller: string;
}

export function validateNsisUninstallPolicy(sources: NsisUninstallSources): void;
