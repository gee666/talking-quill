import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const harness = vi.hoisted(() => ({
  app: {
    enableSandbox: vi.fn(),
    setName: vi.fn(),
    setAppUserModelId: vi.fn(),
    requestSingleInstanceLock: vi.fn(() => true),
    on: vi.fn(),
    whenReady: vi.fn<() => Promise<void>>(),
    quit: vi.fn(),
    exit: vi.fn(),
    isPackaged: false,
  },
  application: vi.fn(),
  registerPrivilegedScheme: vi.fn(),
}));

vi.mock('electron', () => ({
  app: harness.app,
  dialog: { showErrorBox: vi.fn() },
}));
vi.mock('../../app/src/main/app/application', () => ({
  TalkingQuillApplication: harness.application,
}));
vi.mock('../../app/src/main/app/launch-at-login-service', () => ({
  classifyWindowsLoginStartArguments: () => 'normal',
}));
vi.mock('../../app/src/main/app/lifecycle', () => ({
  StartupCancelledError: class StartupCancelledError extends Error {},
  createFatalStartupReport: vi.fn(),
}));
vi.mock('../../app/src/main/app/windows-uninstall-target', () => ({
  resolveSignedInWindowsUserDataTarget: vi.fn(),
}));
vi.mock('../../app/src/main/security/protocol', () => ({
  registerPrivilegedScheme: harness.registerPrivilegedScheme,
}));
vi.mock('../../app/src/main/persistence/paths', () => ({ createAppPaths: vi.fn() }));
vi.mock('../../app/src/main/data/data-lifecycle-service', () => ({
  resetOwnedApplicationData: vi.fn(),
}));
vi.mock('../../app/src/main/data/uninstall-reset-challenge', () => ({
  consumeUninstallResetChallenge: vi.fn(),
}));
vi.mock('../../app/src/main/app/runtime-path-policy', () => ({
  validateUninstallResetTarget: vi.fn(),
}));
vi.mock('../../app/src/main/helper', () => ({ resolveOwnedTreeRemovalExecutable: vi.fn() }));
vi.mock('../../app/src/main/data/native-owned-tree-removal', () => ({
  createNativeOwnedTreeRemoval: vi.fn(),
}));

import { startMain } from '../../app/src/main/bootstrap';

const originalArgv = process.argv;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date('2026-01-01T00:00:00.000Z'));
  process.argv = [...originalArgv, '--talking-quill-request-machine-quit'];
  vi.clearAllMocks();
  harness.app.requestSingleInstanceLock.mockReturnValue(true);
});

afterEach(() => {
  process.argv = originalArgv;
  vi.useRealTimers();
});

describe('bootstrap machine quit', () => {
  it('forces exit at the original deadline when Electron never becomes ready', async () => {
    harness.app.whenReady.mockReturnValue(new Promise(() => undefined));
    const startedAt = Date.now();

    startMain();

    expect(harness.app.quit).toHaveBeenCalledOnce();
    expect(harness.app.quit.mock.invocationCallOrder[0]).toBeLessThan(
      harness.app.whenReady.mock.invocationCallOrder[0] ?? Number.POSITIVE_INFINITY,
    );
    await vi.advanceTimersByTimeAsync(14_999);
    expect(harness.app.exit).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(Date.now()).toBe(startedAt + 15_000);
    expect(harness.app.exit).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('does not schedule quit again when readiness races the initial request', async () => {
    harness.app.whenReady.mockResolvedValue();

    startMain();
    await vi.advanceTimersByTimeAsync(0);

    expect(harness.app.quit).toHaveBeenCalledOnce();
    expect(harness.application).not.toHaveBeenCalled();
    expect(vi.getTimerCount()).toBe(1);

    await vi.advanceTimersByTimeAsync(15_000);
    expect(harness.app.exit).toHaveBeenCalledExactlyOnceWith(0);
  });

  it('queues a second-instance recovery generation until authenticated startup is ready', async () => {
    process.argv = originalArgv.filter(
      (argument) => argument !== '--talking-quill-request-machine-quit',
    );
    let releaseReady: (() => void) | undefined;
    harness.app.whenReady.mockReturnValue(
      new Promise<void>((resolve) => {
        releaseReady = resolve;
      }),
    );
    const application = {
      start: vi.fn(() => Promise.resolve()),
      handleWindowsUpdateRelaunchGeneration: vi.fn(),
      handleApplicationActivation: vi.fn(),
      handleBeforeQuit: vi.fn(),
      quit: vi.fn(),
    };
    harness.application.mockImplementation(function createApplication() {
      return application;
    });

    startMain();
    const secondInstance = harness.app.on.mock.calls.find(
      ([event]) => event === 'second-instance',
    )?.[1] as ((event: unknown, commandLine: string[]) => void) | undefined;
    expect(secondInstance).toBeTypeOf('function');
    const generation = 'cd'.repeat(16);
    secondInstance?.(undefined, [
      'Talking Quill.exe',
      `--windows-update-relaunch-generation-v1=${generation}`,
    ]);
    expect(application.handleWindowsUpdateRelaunchGeneration).not.toHaveBeenCalled();

    releaseReady?.();
    await vi.advanceTimersByTimeAsync(0);

    expect(application.start).toHaveBeenCalledOnce();
    expect(application.handleWindowsUpdateRelaunchGeneration).toHaveBeenCalledExactlyOnceWith(
      generation,
    );
    expect(application.handleApplicationActivation).not.toHaveBeenCalled();
  });
});
