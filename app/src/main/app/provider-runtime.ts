import { isAbsolute } from 'node:path';
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
  readonly environment: NodeJS.ProcessEnv;
  readonly appData: string;
  readonly home: string;
  readonly resolvePiCli?: PiProviderOptions['resolveCli'];
}): ProviderRuntime {
  const credentials = new ProviderCredentialService(options.vault);
  const configs = new ProviderConfigService(options.settings);
  const interactiveAppData = interactivePath(
    options.platform,
    options.environment.TALKING_QUILL_PACKAGED_TEST,
    options.environment.TALKING_QUILL_TEST_INTERACTIVE_APPDATA,
    options.appData,
  );
  const interactiveHome = interactivePath(
    options.platform,
    options.environment.TALKING_QUILL_PACKAGED_TEST,
    options.environment.TALKING_QUILL_TEST_INTERACTIVE_HOME,
    options.home,
  );
  const interactivePaths = {
    ...(interactiveAppData === undefined ? {} : { interactiveAppData }),
    ...(interactiveHome === undefined ? {} : { interactiveHome }),
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

function interactivePath(
  platform: NodeJS.Platform,
  packagedTest: string | undefined,
  testPath: string | undefined,
  electronPath: string,
): string | undefined {
  if (platform !== 'win32') return undefined;
  return packagedTest === '1' && testPath !== undefined && isAbsolute(testPath)
    ? testPath
    : electronPath;
}
