// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { MainApi } from '../../app/src/shared/bridge/api';
import type { AppStatus } from '../../app/src/shared/schemas/app-state';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';
import { PROVIDER_CATALOG } from '../../app/src/main/providers/registry';
import { AppShell } from '../../app/src/renderer/main/AppShell';
import { APP_STATUS_PRESENTATIONS } from '../../app/src/renderer/status-presentation';

const setEnabled = vi.fn<MainApi['app']['setEnabled']>();
const getBootstrap = vi.fn<MainApi['app']['getBootstrap']>();
const updateSettings = vi.fn<MainApi['settings']['update']>();
const toggleMaximize = vi.fn<MainApi['windowControls']['toggleMaximize']>();
const setWelcomeStep = vi.fn<MainApi['welcome']['setStep']>();
const completeWelcome = vi.fn<MainApi['welcome']['complete']>();
const startActivationTest = vi.fn<MainApi['activationTest']['start']>();
const stopActivationTest = vi.fn<MainApi['activationTest']['stop']>();
let activationTestListener: Parameters<MainApi['activationTest']['onChanged']>[0] | null = null;
let settingsListener: Parameters<MainApi['settings']['onChanged']>[0] | null = null;
let maximizeListener: ((maximized: boolean) => void) | null = null;

const readyHelper = {
  status: 'ready',
  reason: null,
  helperVersion: '1.2.3',
  permissions: {
    accessibility: 'not_applicable',
    inputMonitoring: 'not_applicable',
    eventPost: 'not_applicable',
  },
} as const;

const BINDING_TOKEN = '11111111-1111-4111-8111-111111111111';

const api: MainApi = {
  welcome: {
    setStep: setWelcomeStep,
    complete: completeWelcome,
  },
  info: {
    status: () =>
      Promise.resolve({
        microphone: 'not-determined',
        screenRecording: 'unknown',
        helper: readyHelper,
      }),
    checkForUpdates: () =>
      Promise.resolve({
        status: 'current',
        currentVersion: '1.0.0',
        latestVersion: '1.0.0',
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.0.0',
      }),
    cancel: () => Promise.resolve(true),
    openPermissionSettings: () => Promise.resolve(),
    openLocation: () => Promise.resolve(),
    openRelease: () => Promise.resolve(),
    notices: () => Promise.resolve('Third-party notices'),
  },
  activationTest: {
    start: startActivationTest,
    stop: stopActivationTest,
    onChanged: (listener) => {
      activationTestListener = listener;
      return () => {
        activationTestListener = null;
      };
    },
  },
  app: {
    getBootstrap,
    setEnabled,
    onStateChanged: () => () => undefined,
  },
  settings: {
    update: updateSettings,
    onChanged: (listener) => {
      settingsListener = listener;
      return () => {
        settingsListener = null;
      };
    },
  },
  profiles: {
    create: () => Promise.resolve(DEFAULT_SETTINGS),
    update: () => Promise.resolve(DEFAULT_SETTINGS),
    delete: () => Promise.resolve(DEFAULT_SETTINGS),
    reset: () => Promise.resolve(DEFAULT_SETTINGS),
  },
  data: {
    resetAll: () => Promise.resolve(),
    onResetAccepted: () => () => undefined,
  },
  recording: {
    getDevices: () =>
      Promise.resolve({
        devices: [],
        preferredMicrophoneId: null,
        preferredAvailable: true,
        permission: 'not-determined',
      }),
    startTest: () => Promise.resolve({ status: 'idle', permission: 'not-determined' }),
    stopTest: () => Promise.resolve({ status: 'idle', permission: 'not-determined' }),
    openMicrophoneSettings: () => Promise.resolve(),
    onDevicesChanged: () => () => undefined,
    onTestLevel: () => () => undefined,
    onTestStateChanged: () => () => undefined,
  },
  echo: { onSessionChanged: () => () => undefined },
  history: {
    list: () => Promise.resolve({ items: [], nextCursor: null, revision: 0 }),
    delete: () => Promise.resolve({ deleted: true, revision: 1, screenshotCleanup: 'complete' }),
    deleteAll: () =>
      Promise.resolve({ deletedCount: 0, revision: 0, screenshotCleanup: 'complete' }),
    copy: () => Promise.resolve(),
    thumbnail: () => Promise.resolve(null),
    onChanged: () => () => undefined,
  },
  commands: {
    list: () => Promise.resolve([]),
    create: () => Promise.reject(new Error('not used')),
    update: () => Promise.reject(new Error('not used')),
    delete: () => Promise.resolve(false),
    preview: () => Promise.resolve(null),
  },
  vocabulary: {
    list: () => Promise.resolve([]),
    create: () => Promise.reject(new Error('not used')),
    update: () => Promise.reject(new Error('not used')),
    delete: () => Promise.resolve(false),
    importFile: () => Promise.resolve({ status: 'cancelled' }),
    exportFile: () => Promise.resolve({ status: 'cancelled' }),
  },
  providers: {
    catalog: () => Promise.resolve(PROVIDER_CATALOG),
    piInstallationStatus: () =>
      Promise.resolve({
        mode: 'automatic',
        state: 'not-found',
        configuredPath: null,
        path: null,
        version: null,
        source: null,
        errorCode: 'PI_NOT_FOUND',
      }),
    savePiInstallation: () =>
      Promise.resolve({
        mode: 'automatic',
        state: 'not-found',
        configuredPath: null,
        path: null,
        version: null,
        source: null,
        errorCode: 'PI_NOT_FOUND',
      }),
    browsePiInstallation: () => Promise.resolve(null),
    saveConfig: (config) =>
      Promise.resolve({
        settings: {
          ...structuredClone(DEFAULT_SETTINGS),
          smartProcessing: {
            ...structuredClone(DEFAULT_SETTINGS.smartProcessing),
            selectedProviderId: config.providerId,
          },
        },
        credentialState: {
          providerId: config.providerId,
          configured: false,
          updatedAt: null,
          bindingToken: BINDING_TOKEN,
        },
      }),
    setSecret: (providerId) =>
      Promise.resolve({
        providerId,
        configured: true,
        updatedAt: 1,
        bindingToken: BINDING_TOKEN,
      }),
    secretStatus: (providerId) =>
      Promise.resolve({
        providerId,
        configured: false,
        updatedAt: null,
        bindingToken: BINDING_TOKEN,
      }),
    deleteSecret: (providerId) =>
      Promise.resolve({
        providerId,
        configured: false,
        updatedAt: null,
        bindingToken: BINDING_TOKEN,
      }),
    listModels: () => Promise.resolve([]),
    testConnection: () => Promise.resolve({ ok: true, destination: 'local', modelCount: 0 }),
    destination: () => Promise.resolve('local'),
    cancel: () => Promise.resolve(true),
    osaStatus: () =>
      Promise.resolve({
        providerId: 'openai',
        modelId: 'gpt-4.1-nano',
        capability: 'supported',
        manualTestAllowed: false,
        screenPermission: 'granted',
      }),
    setOnScreenAwareness: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
    verifyVision: () => Promise.resolve({ verificationId: '11111111-1111-4111-8111-111111111112' }),
    confirmVision: () => Promise.resolve(structuredClone(DEFAULT_SETTINGS)),
  },
  models: {
    list: () => Promise.resolve([]),
    status: (modelId) =>
      Promise.resolve({
        modelId,
        state: 'missing',
        downloadedBytes: 0,
        totalBytes: 1,
        detail: null,
        repairable: false,
      }),
    download: (modelId) => api.models.status(modelId),
    pause: (modelId) => api.models.status(modelId),
    cancel: (modelId) => api.models.status(modelId),
    retry: (modelId) => api.models.status(modelId),
    delete: async (modelId) => ({ outcome: 'deleted', status: await api.models.status(modelId) }),
    onProgress: () => () => undefined,
  },
  windowControls: {
    minimize: () => Promise.resolve(),
    toggleMaximize,
    close: () => Promise.resolve(),
    onMaximizedChanged: (listener) => {
      maximizeListener = listener;
      return () => {
        maximizeListener = null;
      };
    },
  },
};

afterEach(cleanup);

beforeEach(() => {
  setEnabled.mockReset();
  setEnabled.mockImplementation((enabled) =>
    Promise.resolve({ enabled, status: 'needs-setup', modelReady: false, helper: readyHelper }),
  );
  getBootstrap.mockReset();
  getBootstrap.mockRejectedValue(new Error('bootstrap unavailable'));
  updateSettings.mockReset();
  updateSettings.mockImplementation((patch) => {
    const settings: Settings = {
      ...structuredClone(DEFAULT_SETTINGS),
      app: {
        enabled: patch.app?.enabled ?? DEFAULT_SETTINGS.app.enabled,
        closeToTray: patch.app?.closeToTray ?? DEFAULT_SETTINGS.app.closeToTray,
        activationKey: DEFAULT_SETTINGS.app.activationKey,
        defaultProcessingMode: DEFAULT_SETTINGS.app.defaultProcessingMode,
        widgetSize: patch.app?.widgetSize ?? DEFAULT_SETTINGS.app.widgetSize,
        soundsEnabled: patch.app?.soundsEnabled ?? DEFAULT_SETTINGS.app.soundsEnabled,
        launchAtLogin: patch.app?.launchAtLogin ?? DEFAULT_SETTINGS.app.launchAtLogin,
      },
      recording: {
        preferredMicrophoneId: patch.recording?.preferredMicrophoneId ?? null,
        silencePreset: patch.recording?.silencePreset ?? 'average',
      },
      transcription: {
        modelId: patch.transcription?.modelId ?? DEFAULT_SETTINGS.transcription.modelId,
        language: patch.transcription?.language ?? DEFAULT_SETTINGS.transcription.language,
      },
      privacy: {
        historyEnabled: patch.privacy?.historyEnabled ?? DEFAULT_SETTINGS.privacy.historyEnabled,
        historyRetentionDays:
          patch.privacy?.historyRetentionDays ?? DEFAULT_SETTINGS.privacy.historyRetentionDays,
        retainSmartScreenshots:
          patch.privacy?.retainSmartScreenshots ?? DEFAULT_SETTINGS.privacy.retainSmartScreenshots,
        diagnosticLoggingEnabled:
          patch.privacy?.diagnosticLoggingEnabled ??
          DEFAULT_SETTINGS.privacy.diagnosticLoggingEnabled,
      },
      welcome: {
        ...structuredClone(DEFAULT_SETTINGS.welcome),
        completedAt: 1,
        lastStep: 6,
        microphoneTested: true,
        activationTested: true,
      },
    };
    return Promise.resolve(settings);
  });
  startActivationTest.mockReset();
  startActivationTest.mockResolvedValue({
    active: true,
    phase: 'waiting',
    profileId: null,
    activationKey: null,
    shift: false,
    elapsedMs: 0,
    unavailableReason: null,
  });
  stopActivationTest.mockReset();
  stopActivationTest.mockResolvedValue({
    active: false,
    phase: 'idle',
    profileId: null,
    activationKey: null,
    shift: false,
    elapsedMs: 0,
    unavailableReason: null,
  });
  activationTestListener = null;
  settingsListener = null;
  toggleMaximize.mockReset();
  toggleMaximize.mockResolvedValue(true);
  setWelcomeStep.mockReset();
  setWelcomeStep.mockImplementation((step) =>
    Promise.resolve({
      completedAt: null,
      lastStep: step,
      microphoneTested: false,
      activationTested: false,
      reopened: false,
    }),
  );
  completeWelcome.mockReset();
  completeWelcome.mockResolvedValue({
    completedAt: 1,
    lastStep: 6,
    microphoneTested: true,
    activationTested: true,
    reopened: false,
  });
  maximizeListener = null;
  Object.defineProperty(window, 'talkingQuill', { configurable: true, value: api });
});

function deferred<Value>() {
  let resolve!: (value: Value) => void;
  const promise = new Promise<Value>((resolvePromise) => {
    resolve = resolvePromise;
  });
  return { promise, resolve };
}

function renderShell(
  status: AppStatus = 'needs-setup',
  modelReady = status === 'ready',
  settings = structuredClone(DEFAULT_SETTINGS),
) {
  return render(
    <AppShell
      bootstrap={{
        appVersion: '1.2.3',
        platform: 'win32',
        state: { enabled: true, status, modelReady, helper: readyHelper },
        settings: {
          ...settings,
          welcome: {
            ...settings.welcome,
            completedAt: 1,
            lastStep: 6,
            microphoneTested: true,
            activationTested: true,
            revision: 4,
          },
        },
      }}
    />,
  );
}

describe('main application shell', () => {
  it('transitions from Welcome with the authoritative completion state', async () => {
    const initialWelcome = {
      ...structuredClone(DEFAULT_SETTINGS.welcome),
      lastStep: 6 as const,
    };
    const completed = {
      ...initialWelcome,
      completedAt: 4_321,
      revision: 9,
      reopened: false,
    };
    completeWelcome.mockResolvedValueOnce(completed);
    const user = userEvent.setup();
    render(
      <AppShell
        bootstrap={{
          appVersion: '1.2.3',
          platform: 'win32',
          state: { enabled: true, status: 'ready', modelReady: true, helper: readyHelper },
          settings: { ...structuredClone(DEFAULT_SETTINGS), welcome: initialWelcome },
        }}
      />,
    );

    await user.click(screen.getByRole('button', { name: 'Start using Talking Quill' }));

    expect(completeWelcome).toHaveBeenCalledOnce();
    expect(
      await screen.findByRole('heading', { level: 1, name: 'Talking Quill is ready' }),
    ).toHaveFocus();
  });

  it('ignores stale Welcome snapshots and accepts a newer authoritative rollback', async () => {
    renderShell('ready');
    const stale = structuredClone(DEFAULT_SETTINGS);
    stale.welcome.lastStep = 3;
    stale.welcome.revision = 3;

    act(() => settingsListener?.(stale));

    expect(screen.getByRole('heading', { level: 1, name: 'Talking Quill is ready' })).toBeVisible();
    const rolledBack = structuredClone(stale);
    rolledBack.welcome.revision = 5;
    act(() => settingsListener?.(rolledBack));
    expect(await screen.findByRole('heading', { name: 'Local model' })).toHaveFocus();
  });

  it('offers only Dashboard, Settings, and Info with keyboard navigation and truthful content', async () => {
    const user = userEvent.setup();
    renderShell();
    expect(screen.getAllByRole('button', { name: /Dashboard|Settings|Info/ })).toHaveLength(3);
    const settingsButton = screen.getByRole('button', { name: 'Settings' });
    settingsButton.focus();
    await user.keyboard('{Enter}');
    expect(await screen.findByRole('heading', { name: 'General settings' })).toHaveFocus();
    expect(screen.getByRole('checkbox', { name: 'Close to tray' })).toBeChecked();
    const infoButton = screen.getByRole('button', { name: 'Info' });
    infoButton.focus();
    await user.keyboard('{Enter}');
    expect(await screen.findByRole('heading', { name: 'About Talking Quill' })).toHaveFocus();
    expect(screen.getByText(/no account required/i)).toBeVisible();
    const reopen = screen.getByRole('button', { name: 'Reopen Welcome' });
    await user.click(reopen);
    await user.click(screen.getByRole('button', { name: 'Exit Welcome' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Reopen Welcome' })).toHaveFocus(),
    );
  });

  it('reports provider-managed Text Generation WebUI as ready without a model ID', () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    settings.smartProcessing.selectedProviderId = 'textgenwebui';
    settings.smartProcessing.providers.textgenwebui = {
      baseUrl: 'http://127.0.0.1:5000/v1',
      contextWindow: 4_096,
    };

    renderShell('ready', true, settings);

    expect(screen.getByText('textgenwebui uses its currently loaded model')).toBeVisible();
    expect(screen.queryByText('Needs provider model setup')).not.toBeInTheDocument();
  });

  it('still reports missing required provider models as needing setup', () => {
    const settings = structuredClone(DEFAULT_SETTINGS);
    settings.smartProcessing.selectedProviderId = 'ollama';
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: null,
    };

    renderShell('ready', true, settings);

    expect(screen.getByText('Needs provider model setup')).toBeVisible();
  });

  it('reports model readiness independently from the disabled aggregate status', () => {
    const first = renderShell('disabled', false);
    expect(screen.getAllByText('Needs setup')).toHaveLength(2);
    expect(screen.queryByText('Model available')).not.toBeInTheDocument();

    first.unmount();
    renderShell('disabled', true);
    expect(screen.getByText('Model available')).toBeVisible();
  });

  it.each(Object.entries(APP_STATUS_PRESENTATIONS))(
    'renders the centralized %s status consistently',
    (status, presentation) => {
      renderShell(status as AppStatus);
      expect(screen.getAllByText(presentation.label)).toHaveLength(2);
    },
  );

  it('tracks maximize state with accessible Maximize and Restore labels', async () => {
    const user = userEvent.setup();
    renderShell();
    const maximize = screen.getByRole('button', { name: 'Maximize window' });
    expect(maximize).toHaveAttribute('aria-pressed', 'false');
    await user.click(maximize);
    expect(await screen.findByRole('button', { name: 'Restore window' })).toHaveAttribute(
      'aria-pressed',
      'true',
    );
    maximizeListener?.(false);
    expect(await screen.findByRole('button', { name: 'Maximize window' })).toBeVisible();
  });

  it('restores authoritative application state and reports rejected enabled writes', async () => {
    const user = userEvent.setup();
    setEnabled.mockRejectedValueOnce(new Error('secret persistence detail'));
    renderShell();

    const enabled = screen.getByRole('checkbox', { name: 'Talking Quill enabled' });
    await user.click(enabled);
    await waitFor(() => expect(enabled).toBeChecked());
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Talking Quill could not save the enabled setting.',
    );
    expect(screen.queryByText(/secret persistence detail/i)).not.toBeInTheDocument();
  });

  it('runs the safe helper activation test and renders Quick and Extended results', async () => {
    const user = userEvent.setup();
    renderShell();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Test activation shortcut' }));
    expect(startActivationTest).toHaveBeenCalledOnce();
    expect(screen.getByText('Waiting for shortcut')).toBeVisible();

    act(() =>
      activationTestListener?.({
        active: true,
        phase: 'quick',
        profileId: null,
        activationKey: null,
        shift: false,
        elapsedMs: 200,
        unavailableReason: null,
      }),
    );
    expect(await screen.findByText('Quick Dictation gesture recognized')).toBeVisible();
    act(() =>
      activationTestListener?.({
        active: true,
        phase: 'extended',
        profileId: 'prompt',
        activationKey: 'Z',
        shift: true,
        elapsedMs: 600,
        unavailableReason: null,
      }),
    );
    expect(
      await screen.findByText(
        'Extended Dictation gesture recognized: Prompt, Alt/Option + Shift + Z',
      ),
    ).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Stop shortcut test' }));
    expect(stopActivationTest).toHaveBeenCalled();
    act(() =>
      activationTestListener?.({
        active: false,
        phase: 'idle',
        profileId: null,
        activationKey: null,
        shift: false,
        elapsedMs: 0,
        unavailableReason: 'helper-unavailable',
      }),
    );
    expect(
      await screen.findByText('The native keyboard helper must be ready before testing'),
    ).toBeVisible();
  });

  it('restores authoritative Settings state and reports rejected enabled writes', async () => {
    const user = userEvent.setup();
    updateSettings.mockRejectedValueOnce(new Error('secret persistence detail'));
    renderShell();
    await user.click(screen.getByRole('button', { name: 'Settings' }));

    const enabled = await screen.findByRole('checkbox', { name: 'Enable Talking Quill' });
    await user.click(enabled);
    await waitFor(() => expect(enabled).toBeChecked());
    expect(await screen.findByRole('alert')).toHaveTextContent('The setting could not be saved.');
    expect(screen.queryByText(/secret persistence detail/i)).not.toBeInTheDocument();
  });

  it('reloads the authoritative full settings snapshot after a rejected General save', async () => {
    const authoritative = structuredClone(DEFAULT_SETTINGS);
    const authoritativeGeneral = authoritative.dictationProfiles[0];
    if (authoritativeGeneral === undefined) throw new Error('Default General profile is missing');
    authoritativeGeneral.activationKey = 'Q';
    authoritative.app.closeToTray = true;
    authoritative.welcome.completedAt = 1;
    authoritative.welcome.lastStep = 6;
    getBootstrap.mockResolvedValueOnce({
      appVersion: '1.2.3',
      platform: 'win32',
      state: { enabled: true, status: 'needs-setup', modelReady: false, helper: readyHelper },
      settings: authoritative,
    });
    updateSettings.mockRejectedValueOnce(new Error('ambiguous persistence failure'));
    const user = userEvent.setup();
    renderShell();
    await user.click(screen.getByRole('button', { name: 'Settings' }));

    await user.click(await screen.findByRole('checkbox', { name: 'Close to tray' }));

    await waitFor(() =>
      expect(screen.getByRole('checkbox', { name: 'Close to tray' })).toBeChecked(),
    );
    expect(await screen.findByRole('alert')).toHaveTextContent('The setting could not be saved.');
    await user.click(screen.getByRole('button', { name: 'Dictation profiles' }));
    expect(screen.getByRole('combobox', { name: 'Activation key' })).toHaveValue('Q');
  });

  it('does not overwrite a newer settings event with an older failed-save reload', async () => {
    const reload = deferred<Awaited<ReturnType<MainApi['app']['getBootstrap']>>>();
    getBootstrap.mockReturnValueOnce(reload.promise);
    updateSettings.mockRejectedValueOnce(new Error('save failed'));
    const user = userEvent.setup();
    renderShell();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('checkbox', { name: 'Close to tray' }));
    await vi.waitFor(() => expect(getBootstrap).toHaveBeenCalledOnce());

    const newer = structuredClone(DEFAULT_SETTINGS);
    const newerGeneral = newer.dictationProfiles[0];
    if (newerGeneral === undefined) throw new Error('Default General profile is missing');
    newerGeneral.activationKey = 'X';
    newer.welcome.completedAt = 1;
    newer.welcome.lastStep = 6;
    act(() => settingsListener?.(newer));
    reload.resolve({
      appVersion: '1.2.3',
      platform: 'win32',
      state: { enabled: true, status: 'needs-setup', modelReady: false, helper: readyHelper },
      settings: structuredClone(DEFAULT_SETTINGS),
    });

    await user.click(screen.getByRole('button', { name: 'Dictation profiles' }));
    await waitFor(() =>
      expect(screen.getByRole('combobox', { name: 'Activation key' })).toHaveValue('X'),
    );
  });

  it('restores authoritative close behavior and reports rejected writes', async () => {
    const user = userEvent.setup();
    updateSettings.mockRejectedValueOnce(new Error('secret persistence detail'));
    renderShell();
    await user.click(screen.getByRole('button', { name: 'Settings' }));

    const closeToTray = await screen.findByRole('checkbox', { name: 'Close to tray' });
    await user.click(closeToTray);
    await waitFor(() => expect(closeToTray).toBeChecked());
    expect(await screen.findByRole('alert')).toHaveTextContent('The setting could not be saved.');
    expect(screen.queryByText(/secret persistence detail/i)).not.toBeInTheDocument();
  });
});
