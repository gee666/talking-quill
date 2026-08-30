export interface NsisUninstallSources {
  readonly custom: string;
  readonly assisted: string;
  readonly uninstaller: string;
  readonly installer: string;
  readonly installSection: string;
  readonly installUtil: string;
  readonly installerInclude: string;
  readonly common: string;
  readonly extractAppPackage: string;
  readonly oneInstance: string;
  readonly multiUserUi: string;
  readonly installValidation: string;
  readonly cleanup: string;
}

export function validateNsisUninstallPolicy(sources: NsisUninstallSources): void;
