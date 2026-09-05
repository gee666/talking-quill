import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { readFileSync } from 'node:fs';
import { transformWindowsInstalledAcceptanceSource } from '../../app/windows-installed-acceptance-overlay';

const stages = vi.hoisted(() => ({
  foundation: vi.fn(),
  services: vi.fn(),
  interaction: vi.fn(),
  completion: vi.fn(),
}));
vi.mock('electron', () => ({ app: { isPackaged: false } }));
vi.mock('../../app/src/main/app/application-startup-foundation', () => ({
  prepareFoundation: stages.foundation,
}));
vi.mock('../../app/src/main/app/application-startup-services', () => ({
  prepareServices: stages.services,
}));
vi.mock('../../app/src/main/app/application-startup-interaction', () => ({
  prepareInteraction: stages.interaction,
}));
vi.mock('../../app/src/main/app/application-startup-completion', () => ({
  completeStartup: stages.completion,
}));

import {
  ApplicationRuntime,
  type ApplicationLifecycle,
} from '../../app/src/main/app/application-runtime';
import {
  startApplication,
  type ApplicationStartupHooks,
} from '../../app/src/main/app/application-startup';
import type { ApplicationShutdown } from '../../app/src/main/app/application-shutdown';
import type { ApplicationReset } from '../../app/src/main/app/application-reset';
import { StartupCancelledError, type StartupCleanupStack } from '../../app/src/main/app/lifecycle';
import { WindowRoleRegistry } from '../../app/src/main/app/window-role-registry';

function createRuntime(): ApplicationRuntime {
  let lifecycle: ApplicationLifecycle = 'starting';
  return new ApplicationRuntime({}, new WindowRoleRegistry(), {
    getLifecycle: () => lifecycle,
    setLifecycle: (next) => {
      lifecycle = next;
    },
    getHelper: () => null,
    setHelper: vi.fn(),
    getSettings: () => null,
    setSettings: vi.fn(),
    getWindows: () => null,
    setWindows: vi.fn(),
    getDiagnostics: () => null,
    setDiagnostics: vi.fn(),
  });
}

const hooks: ApplicationStartupHooks = {
  helperExecutablePath: () => null,
  resumeMacosCleanup: vi.fn(),
  validInstalledMacosOwner: () => false,
  acknowledgeWindowsUpdateRelaunches: vi.fn(),
  testQuitRequest: vi.fn(),
};
// Stage mocks never call these controllers; their identity must pass through unchanged.
const shutdown = {} as ApplicationShutdown;
const reset = {} as ApplicationReset;

beforeEach(() => vi.resetAllMocks());
afterEach(() => vi.restoreAllMocks());

describe('application startup composition', () => {
  it('passes each acquired stage forward and disarms rollback after completion', async () => {
    const runtime = createRuntime();
    const foundation = { stage: 'foundation' };
    const services = { stage: 'services' };
    const interaction = { stage: 'interaction' };
    const dispose = vi.fn();
    let stack!: StartupCleanupStack;
    stages.foundation.mockImplementation((_runtime, cleanup: StartupCleanupStack) => {
      stack = cleanup;
      cleanup.add('foundation', dispose);
      return Promise.resolve(foundation);
    });
    stages.services.mockResolvedValue(services);
    stages.interaction.mockReturnValue(interaction);
    stages.completion.mockResolvedValue(undefined);

    await startApplication(runtime, hooks, shutdown, reset);

    expect(stages.services).toHaveBeenCalledWith(runtime, stack, hooks, shutdown, foundation);
    expect(stages.interaction).toHaveBeenCalledWith(
      runtime,
      stack,
      shutdown,
      reset,
      foundation,
      services,
    );
    expect(stages.completion).toHaveBeenCalledWith(
      runtime,
      stack,
      hooks,
      shutdown,
      foundation,
      services,
      interaction,
    );
    await stack.rollback();
    expect(dispose).not.toHaveBeenCalled();
  });

  it.each(['foundation', 'services', 'interaction', 'completion'] as const)(
    'rolls back the acquired prefix when %s fails',
    async (failedStage) => {
      const runtime = createRuntime();
      const failure = new Error('injected stage failure');
      const acquired: string[] = [];
      const disposed: string[] = [];
      for (const [name, stage] of Object.entries(stages)) {
        stage.mockImplementation((_runtime, cleanup: StartupCleanupStack) => {
          acquired.push(name);
          cleanup.add(name, () => {
            disposed.push(name);
          });
          if (name === failedStage) throw failure;
          return {};
        });
      }

      await expect(startApplication(runtime, hooks, shutdown, reset)).rejects.toBe(failure);

      expect(disposed).toEqual([...acquired].reverse());
      expect(acquired.at(-1)).toBe(failedStage);
      expect(runtime.lifecycle).toBe('failed');
      expect(runtime.runtimeDisposers).toHaveLength(0);
    },
  );

  it('keeps cancellation distinct from failure and disposes runtime subscriptions once', async () => {
    const runtime = createRuntime();
    const dispose = vi.fn();
    let stack!: StartupCleanupStack;
    stages.foundation.mockImplementation((_runtime, cleanup: StartupCleanupStack) => {
      stack = cleanup;
      runtime.ownRuntimeDisposer(cleanup, 'subscription', dispose);
      runtime.lifecycle = 'stopping';
      runtime.startupAbort.abort();
      runtime.assertStartupActive();
    });

    await expect(startApplication(runtime, hooks, shutdown, reset)).rejects.toBeInstanceOf(
      StartupCancelledError,
    );
    await stack.rollback();
    expect(runtime.lifecycle).toBe('stopped');
    expect(dispose).toHaveBeenCalledOnce();
    expect(stages.services).not.toHaveBeenCalled();
  });

  it('retains the application anchors required by the installed-acceptance overlay', () => {
    const source = readFileSync('app/src/main/app/application.ts', 'utf8');
    const transformed = transformWindowsInstalledAcceptanceSource('application', source);
    expect(transformed).toContain('this.#helper as InstalledAcceptanceHelper');
    expect(transformed).toContain('async runInstalledAcceptance(');
    for (const field of ['helper', 'settings', 'windows', 'diagnostics', 'lifecycle']) {
      expect(source).toMatch(new RegExp(`#${field}:`));
    }
  });
});
