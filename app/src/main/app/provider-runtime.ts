import type { EgressObserver } from '../security/egress-audit';
import type { CredentialVault } from '../persistence/credential-vault';
import type { SettingsStore } from '../persistence/settings-store';
import {
  PiInstallationService,
  PinnedJsonTransport,
  ProviderConfigService,
  ProviderCredentialService,
  ProviderMutationService,
  ProviderRegistry,
  ProviderService,
} from '../providers';
import type { PiProviderOptions } from '../providers/pi';

export interface ProviderRuntime {
  readonly configs: ProviderConfigService;
  readonly piInstallation: PiInstallationService;
  readonly providers: ProviderService;
  createMutations(): ProviderMutationService;
}

export function createProviderRuntime(options: {
  readonly settings: SettingsStore;
  readonly vault: CredentialVault;
  readonly workingDirectory: string;
  readonly observeEgress: EgressObserver;
  readonly platform: NodeJS.Platform;
  readonly interactiveAppData?: string;
  readonly interactiveHome?: string;
  readonly resolvePiCli?: PiProviderOptions['resolveCli'];
  /** Optional enum-only Pi RPC timing sink; never receives prompts or provider configuration. */
  readonly onPiRpcTiming?: PiProviderOptions['onRpcTiming'];
}): ProviderRuntime {
  const credentials = new ProviderCredentialService(options.vault);
  const configs = new ProviderConfigService(options.settings);
  const interactivePaths = {
    ...(options.interactiveAppData === undefined
      ? {}
      : { interactiveAppData: options.interactiveAppData }),
    ...(options.interactiveHome === undefined ? {} : { interactiveHome: options.interactiveHome }),
  };
  const piInstallation = new PiInstallationService(options.settings, interactivePaths);
  const providers = new ProviderService(
    new ProviderRegistry({
      transport: new PinnedJsonTransport(undefined, {
        category: 'provider',
        observeEgress: options.observeEgress,
      }),
      pi: {
        observeEgress: options.observeEgress,
        workingDirectory: options.workingDirectory,
        configuredPath: () => piInstallation.configuredPath(),
        ...interactivePaths,
        ...(options.resolvePiCli === undefined ? {} : { resolveCli: options.resolvePiCli }),
        ...(options.onPiRpcTiming === undefined ? {} : { onRpcTiming: options.onPiRpcTiming }),
      },
    }),
    credentials,
  );
  return {
    configs,
    piInstallation,
    providers,
    createMutations: () => new ProviderMutationService(configs, credentials, providers),
  };
}
