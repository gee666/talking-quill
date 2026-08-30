import { mkdir } from 'node:fs/promises';
import { join } from 'node:path';
import { afterEach, describe, expect, it, vi } from 'vitest';
import type * as PersistenceModule from '../../app/src/main/persistence';
import { INITIAL_HELPER_READINESS } from '../../app/src/shared/schemas/helper-readiness';
import { createTestDirectory, removeTestDirectory } from '../helpers/temp';

const testState = vi.hoisted(() => ({
  paths: new Map<string, string>(),
  windowFailure: new Error('injected window startup failure'),
}));

vi.mock('electron', () => ({
  app: {
    isPackaged: false,
    getPath: (name: string) => testState.paths.get(name) ?? '',
    getAppPath: () => process.cwd(),
    getVersion: () => '0.0.48',
    getLoginItemSettings: () => ({ wasOpenedAtLogin: false, openAtLogin: false }),
    setLoginItemSettings: vi.fn(),
  },
  clipboard: { writeText: vi.fn() },
  powerMonitor: { on: vi.fn(), removeListener: vi.fn() },
  safeStorage: {
    isEncryptionAvailable: () => true,
    encryptString: (value: string) => Buffer.from(value),
    decryptString: (value: Buffer) => value.toString('utf8'),
  },
  session: {
    fromPartition: () => ({ protocol: {}, webRequest: {} }),
  },
  shell: { beep: vi.fn(), openExternal: vi.fn(), trashItem: vi.fn() },
}));

vi.mock('../../app/src/main/data/data-lifecycle-service', () => ({
  DataLifecycleService: class {
    readonly resetPrepared = false;
    reconcileCopiedProfile = vi.fn();
    recoverPendingReset = vi.fn();
    initializeOwnership = vi.fn();
  },
}));

vi.mock('../../app/src/main/persistence', async (importOriginal) => {
  const original = await importOriginal<typeof PersistenceModule>();
  return {
    ...original,
    HistoryStore: class {
      close = vi.fn();
    },
    CredentialVault: class {
      initialize = vi.fn();
      flush = vi.fn();
    },
  };
});

vi.mock('../../app/src/main/providers', () => ({
  PinnedJsonTransport: class {
    readonly testStub = true;
  },
  ProviderOperationCoordinator: class {
    dispose = vi.fn();
  },
}));

vi.mock('../../app/src/main/app/provider-runtime', () => ({
  createProviderRuntime: () => ({
    configs: {},
    piInstallation: {},
    providers: { dispose: vi.fn(), drain: vi.fn() },
    createMutations: () => ({
      reconcileAll: vi.fn(),
      stopAccepting: vi.fn(),
      drain: vi.fn(),
    }),
  }),
}));

vi.mock('../../app/src/main/security/protocol', () => ({
  installApplicationProtocol: () => vi.fn(),
}));
vi.mock('../../app/src/main/security/session-policy', () => ({
  getTrustedCaptureDocument: () => null,
  secureSession: () => vi.fn(),
}));
vi.mock('../../app/src/main/security/microphone-permission', () => ({
  MicrophonePermissionController: class {
    openSettings = vi.fn();
  },
}));
vi.mock('../../app/src/main/security/system-audio-capture', () => ({
  SystemAudioCaptureController: class {
    dispose = vi.fn();
  },
}));

vi.mock('../../app/src/main/transcription', () => ({
  ModelAccessCoordinator: class {
    readonly testStub = true;
  },
  ModelManager: class {
    initialize = vi.fn();
    shutdown = vi.fn();
  },
  WhisperClientError: class extends Error {},
  WhisperWorkerClient: class {
    close = vi.fn();
  },
}));
vi.mock('../../app/src/main/app/model-runtime-coordinator', () => ({
  ModelRuntimeCoordinator: class {
    subscribeProgress = () => vi.fn();
    bindState = () => Promise.resolve({ state: 'missing' });
  },
}));
vi.mock('../../app/src/main/audio/recording-service', () => ({
  RecordingService: class {
    shutdown = vi.fn();
  },
}));
vi.mock('../../app/src/main/audio/capture-window-client', () => ({
  CaptureWindowClient: class {
    readonly testStub = true;
  },
}));

vi.mock('../../app/src/main/app/source-e2e-harness', () => ({
  SourceE2EHarness: class {
    loadVocabularyDialogs = () => undefined;
    piResolverOverride = () => undefined;
    loadTask6 = () => null;
    testNow = () => null;
  },
}));
vi.mock('../../app/src/main/app/launch-at-login-service', () => ({
  LaunchAtLoginService: class {
    reconcile = vi.fn();
    dispose = vi.fn();
  },
}));
vi.mock('../../app/src/main/app/window-manager', () => ({
  WindowManager: class {
    readonly testStub = true;

    constructor() {
      throw testState.windowFailure;
    }
  },
}));

vi.mock('../../app/src/main/helper/helper-input-device-router', () => ({
  installHelperInputDeviceRouter: () => vi.fn(),
}));
vi.mock('../../app/src/main/helper/helper-wake-revalidator', () => ({
  installHelperWakeRevalidator: () => vi.fn(),
}));
vi.mock('../../app/src/main/helper', () => {
  class HelperClient {
    readiness = INITIAL_HELPER_READINESS;
    nativeLaunchFailure: string | null = null;
    readonly listeners = new Set<(readiness: typeof INITIAL_HELPER_READINESS) => void>();

    start(): Promise<void> {
      return Promise.resolve();
    }

    subscribeReadiness(listener: (readiness: typeof INITIAL_HELPER_READINESS) => void): () => void {
      this.listeners.add(listener);
      return () => this.listeners.delete(listener);
    }

    stop(): Promise<void> {
      this.nativeLaunchFailure = 'authority-rejected';
      this.readiness = {
        ...INITIAL_HELPER_READINESS,
        status: 'unavailable',
        reason: 'owner-auth-failed',
      };
      for (const listener of this.listeners) listener(this.readiness);
      return Promise.resolve();
    }
  }
  return {
    HelperClient,
    activationCaptureRollbackEnabled: () => false,
    resolveHelperExecutable: () => 'fake-helper.exe',
  };
});

import { TalkingQuillApplication } from '../../app/src/main/app/application';
import { DiagnosticLogger } from '../../app/src/main/security/diagnostic-logger';

const owned: string[] = [];

afterEach(async () => {
  vi.restoreAllMocks();
  testState.paths.clear();
  await Promise.all(owned.splice(0).map((path) => removeTestDirectory(path)));
});

describe('TalkingQuillApplication operational diagnostics', () => {
  it('does not let never-settling diagnostic initialization block startup or rollback', async () => {
    const root = await createTestDirectory('application-diagnostic-init-timeout');
    owned.push(root);
    const appData = join(root, 'app-data');
    const home = join(root, 'home');
    const temporary = join(root, 'temp');
    await Promise.all([mkdir(appData), mkdir(home), mkdir(temporary)]);
    testState.paths.set('appData', appData);
    testState.paths.set('userData', join(appData, 'Talking Quill'));
    testState.paths.set('home', home);
    testState.paths.set('temp', temporary);

    const initialize = vi
      .spyOn(DiagnosticLogger.prototype, 'initialize')
      .mockImplementationOnce(() => new Promise<void>(() => undefined));
    const application = new TalkingQuillApplication();
    await expect(application.start()).rejects.toBe(testState.windowFailure);
    expect(initialize).toHaveBeenCalledOnce();
  });

  it('does not let a permanently blocked diagnostic write strand startup rollback', async () => {
    const root = await createTestDirectory('application-operational-diagnostics');
    owned.push(root);
    const appData = join(root, 'app-data');
    const home = join(root, 'home');
    const temporary = join(root, 'temp');
    await Promise.all([mkdir(appData), mkdir(home), mkdir(temporary)]);
    testState.paths.set('appData', appData);
    testState.paths.set('userData', join(appData, 'Talking Quill'));
    testState.paths.set('home', home);
    testState.paths.set('temp', temporary);

    const initialize = vi
      .spyOn(DiagnosticLogger.prototype, 'initialize')
      .mockRejectedValueOnce(new Error('injected diagnostic initialization failure'));
    const recordStartupFailure = vi
      .spyOn(DiagnosticLogger.prototype, 'recordStartupFailure')
      .mockImplementationOnce(() => new Promise<boolean>(() => undefined));

    const application = new TalkingQuillApplication();
    const startup = application.start();
    const rejected = expect(startup).rejects.toBe(testState.windowFailure);
    await vi.waitFor(() => expect(recordStartupFailure).toHaveBeenCalledOnce());
    expect(initialize).toHaveBeenCalledOnce();

    await rejected;
  });
});
