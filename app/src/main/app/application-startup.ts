import { StartupCleanupStack, reportLifecycleDiagnostics } from './lifecycle';
import type { ApplicationRuntime } from './application-runtime';
import { LIFECYCLE_TIMEOUT_MS } from './application-runtime';
import type { ApplicationShutdown } from './application-shutdown';
import type { ApplicationReset } from './application-reset';
import { prepareFoundation } from './application-startup-foundation';
import { prepareServices } from './application-startup-services';
import { prepareInteraction } from './application-startup-interaction';
import { completeStartup } from './application-startup-completion';
export interface ApplicationStartupHooks {
  readonly helperExecutablePath: () => string | null;
  readonly resumeMacosCleanup: (helperExecutablePath: string | null) => void;
  readonly validInstalledMacosOwner: (resourcesPath: string, helperExecutable: string) => boolean;
  readonly acknowledgeWindowsUpdateRelaunches: () => void;
  readonly testQuitRequest: () => void;
}

export async function startApplication(
  runtime: ApplicationRuntime,
  hooks: ApplicationStartupHooks,
  shutdown: ApplicationShutdown,
  reset: ApplicationReset,
): Promise<void> {
  const cleanup = new StartupCleanupStack();
  try {
    const foundation = await prepareFoundation(runtime, cleanup, hooks);
    const services = await prepareServices(runtime, cleanup, hooks, shutdown, foundation);
    const interaction = prepareInteraction(runtime, cleanup, shutdown, reset, foundation, services);
    await completeStartup(runtime, cleanup, hooks, shutdown, foundation, services, interaction);
    cleanup.disarm();
  } catch (error: unknown) {
    runtime.lifecycle = runtime.lifecycle === 'stopping' ? 'stopped' : 'failed';
    reportLifecycleDiagnostics(await cleanup.rollback(LIFECYCLE_TIMEOUT_MS));
    runtime.clearOwnedReferences();
    throw error;
  }
}
