// @vitest-environment jsdom
import { cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PROVIDER_CATALOG } from '../../app/src/main/providers/registry';
import { AppShell } from '../../app/src/renderer/main/AppShell';
import { reconcileDiscoveredModels } from '../../app/src/renderer/main/pi-model-selection';
import type { MainApi } from '../../app/src/shared/bridge/api';
import type { ProviderCredentialState } from '../../app/src/shared/schemas/credentials';
import type { ProviderConfig } from '../../app/src/shared/schemas/providers';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

const saveConfig = vi.fn<MainApi['providers']['saveConfig']>();
const setSecret = vi.fn<MainApi['providers']['setSecret']>();
const secretStatus = vi.fn<MainApi['providers']['secretStatus']>();
const deleteSecret = vi.fn<MainApi['providers']['deleteSecret']>();
const listModels = vi.fn<MainApi['providers']['listModels']>();
const testConnection = vi.fn<MainApi['providers']['testConnection']>();
const destination = vi.fn<MainApi['providers']['destination']>();
const cancel = vi.fn<MainApi['providers']['cancel']>();
const osaStatus = vi.fn<MainApi['providers']['osaStatus']>();
const setOnScreenAwareness = vi.fn<MainApi['providers']['setOnScreenAwareness']>();
const verifyVision = vi.fn<MainApi['providers']['verifyVision']>();
const confirmVision = vi.fn<MainApi['providers']['confirmVision']>();
const BINDING_TOKEN = '11111111-1111-4111-8111-111111111111';

let settings = structuredClone(DEFAULT_SETTINGS);
let settingsListener: ((next: Settings) => void) | null = null;
const helper = {
  status: 'ready',
  reason: null,
  helperVersion: '1.0.0',
  permissions: {
    accessibility: 'not_applicable',
    inputMonitoring: 'not_applicable',
    eventPost: 'not_applicable',
  },
} as const;

const api: MainApi = {
  welcome: {
    setStep: (step) =>
      Promise.resolve({
        completedAt: null,
        lastStep: step,
        microphoneTested: false,
        activationTested: false,
        reopened: false,
      }),
    complete: () =>
      Promise.resolve({
        completedAt: 1,
        lastStep: 6,
        microphoneTested: true,
        activationTested: true,
        reopened: false,
      }),
  },
  info: {
    status: () =>
      Promise.resolve({ microphone: 'not-determined', screenRecording: 'unknown', helper }),
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
    start: () =>
      Promise.resolve({
        active: true,
        phase: 'waiting',
        profileId: null,
        activationKey: null,
        shift: false,
        elapsedMs: 0,
        unavailableReason: null,
      }),
    stop: () =>
      Promise.resolve({
        active: false,
        phase: 'idle',
        profileId: null,
        activationKey: null,
        shift: false,
        elapsedMs: 0,
        unavailableReason: null,
      }),
    onChanged: () => () => undefined,
  },
  app: {
    getBootstrap: vi.fn(),
    setEnabled: (enabled) =>
      Promise.resolve({ enabled, status: 'needs-setup', modelReady: false, helper }),
    onStateChanged: () => () => undefined,
  },
  settings: {
    update: (patch) => {
      settings = {
        ...settings,
        app: {
          enabled: patch.app?.enabled ?? settings.app.enabled,
          closeToTray: patch.app?.closeToTray ?? settings.app.closeToTray,
          activationKey: settings.app.activationKey,
          defaultProcessingMode: settings.app.defaultProcessingMode,
          widgetSize: patch.app?.widgetSize ?? settings.app.widgetSize,
          soundsEnabled: patch.app?.soundsEnabled ?? settings.app.soundsEnabled,
          launchAtLogin: patch.app?.launchAtLogin ?? settings.app.launchAtLogin,
        },
        recording: {
          preferredMicrophoneId:
            patch.recording?.preferredMicrophoneId ?? settings.recording.preferredMicrophoneId,
          silencePreset: patch.recording?.silencePreset ?? settings.recording.silencePreset,
        },
        transcription: {
          modelId: patch.transcription?.modelId ?? settings.transcription.modelId,
          language: patch.transcription?.language ?? settings.transcription.language,
        },
      };
      settingsListener?.(settings);
      return Promise.resolve(settings);
    },
    onChanged: (listener) => {
      settingsListener = listener;
      return () => {
        settingsListener = null;
      };
    },
  },
  profiles: {
    create: () => Promise.resolve(settings),
    update: () => Promise.resolve(settings),
    delete: () => Promise.resolve(settings),
    reset: () => Promise.resolve(settings),
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
    browsePiInstallation: () =>
      Promise.resolve({
        mode: 'automatic',
        state: 'not-found',
        configuredPath: null,
        path: null,
        version: null,
        source: null,
        errorCode: 'PI_NOT_FOUND',
      }),
    saveConfig,
    setSecret,
    secretStatus,
    deleteSecret,
    listModels,
    testConnection,
    destination,
    cancel,
    osaStatus,
    setOnScreenAwareness,
    verifyVision,
    confirmVision,
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
    toggleMaximize: () => Promise.resolve(false),
    close: () => Promise.resolve(),
    onMaximizedChanged: () => () => undefined,
  },
};

beforeEach(() => {
  settings = structuredClone(DEFAULT_SETTINGS);
  settings.welcome = {
    completedAt: 1,
    lastStep: 6,
    microphoneTested: true,
    activationTested: true,
  };
  settingsListener = null;
  saveConfig.mockReset();
  saveConfig.mockImplementation((config) => {
    settings = settingsWithConfig(settings, config);
    settingsListener?.(settings);
    return Promise.resolve({
      settings,
      credentialState: {
        providerId: config.providerId,
        configured: false,
        updatedAt: null,
        bindingToken: BINDING_TOKEN,
      },
    });
  });
  setSecret.mockReset();
  setSecret.mockImplementation((providerId) =>
    Promise.resolve({
      providerId,
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    }),
  );
  secretStatus.mockReset();
  secretStatus.mockImplementation((providerId) =>
    Promise.resolve({
      providerId,
      configured: false,
      updatedAt: null,
      bindingToken: BINDING_TOKEN,
    }),
  );
  deleteSecret.mockReset();
  deleteSecret.mockImplementation((providerId) =>
    Promise.resolve({
      providerId,
      configured: false,
      updatedAt: null,
      bindingToken: BINDING_TOKEN,
    }),
  );
  listModels.mockReset();
  listModels.mockResolvedValue([
    { id: 'llama3.2', name: 'Llama 3.2', contextWindow: 8_192, vision: 'unsupported' },
  ]);
  testConnection.mockReset();
  testConnection.mockResolvedValue({ ok: true, destination: 'local', modelCount: 1 });
  destination.mockReset();
  destination.mockResolvedValue('local');
  cancel.mockReset();
  cancel.mockResolvedValue(true);
  osaStatus.mockReset();
  osaStatus.mockResolvedValue({
    providerId: 'openai',
    modelId: 'gpt-4.1-nano',
    capability: 'supported',
    manualTestAllowed: false,
    screenPermission: 'granted',
  });
  setOnScreenAwareness.mockReset();
  setOnScreenAwareness.mockResolvedValue(structuredClone(DEFAULT_SETTINGS));
  verifyVision.mockReset();
  verifyVision.mockResolvedValue({ verificationId: '11111111-1111-4111-8111-111111111112' });
  confirmVision.mockReset();
  confirmVision.mockResolvedValue(structuredClone(DEFAULT_SETTINGS));
  Object.defineProperty(window, 'talkingQuill', { configurable: true, value: api });
});

afterEach(cleanup);

function renderSettings() {
  render(
    <AppShell
      bootstrap={{
        appVersion: '1.0.0',
        platform: 'win32',
        state: { enabled: true, status: 'needs-setup', modelReady: false, helper },
        settings,
      }}
    />,
  );
  return userEvent.setup();
}

describe('Pi model selection reconciliation', () => {
  const first = {
    id: 'p/one',
    name: 'p/one',
    contextWindow: 8_000,
    vision: 'unsupported' as const,
  };
  const second = {
    id: 'p/two',
    name: 'p/two',
    contextWindow: 8_000,
    vision: 'unsupported' as const,
  };

  it('preserves exact selections and never silently selects or clears a model', () => {
    expect(
      reconcileDiscoveredModels([first, second], { modelId: 'p/two' }, true).draft.modelId,
    ).toBe('p/two');
    expect(reconcileDiscoveredModels([first], { modelId: 'p/two' }, true).draft.modelId).toBe(
      'p/two',
    );
    expect(reconcileDiscoveredModels([first], {}, true).draft.modelId).toBeUndefined();
    expect(reconcileDiscoveredModels([first, second], {}, true).draft.modelId).toBeUndefined();
    const empty = reconcileDiscoveredModels([], { modelId: 'p/two' }, true);
    expect(empty.draft.modelId).toBe('p/two');
    expect(empty.message).toMatch(/exact saved model is retained/u);
  });
});

describe('Smart processing settings', () => {
  it('keeps unknown vision off and cancels disclosed live image-echo verification', async () => {
    const pendingVision: {
      resolve: ((value: { readonly verificationId: string }) => void) | null;
    } = { resolve: null };
    verifyVision.mockImplementation(
      () =>
        new Promise<{ readonly verificationId: string }>((resolve) => {
          pendingVision.resolve = resolve;
        }),
    );
    osaStatus.mockResolvedValue({
      providerId: 'generic-openai',
      modelId: 'private-model',
      capability: 'unknown',
      manualTestAllowed: true,
      screenPermission: 'granted',
    });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Vision support is unknown and remains off.')).toBeVisible();
    expect(screen.queryByRole('checkbox', { name: /focused display/i })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Run disclosed image-echo test' }));
    expect(screen.getByRole('dialog', { name: 'Live image-echo verification' })).toHaveTextContent(
      'sends that one screenshot to the configured provider. No image is retained',
    );
    await user.click(screen.getByRole('button', { name: 'Capture and verify' }));
    await user.click(screen.getByRole('button', { name: /^Cancel$/ }));
    expect(cancel).toHaveBeenCalledWith(expect.stringContaining('-vision-'));
    if (pendingVision.resolve === null) throw new Error('Vision verification was not pending');
    pendingVision.resolve({ verificationId: '11111111-1111-4111-8111-111111111113' });
    await waitFor(() => expect(verifyVision).toHaveBeenCalledOnce());
    expect(confirmVision).not.toHaveBeenCalled();
    expect(screen.queryByRole('dialog', { name: 'Live image-echo verification' })).toBeNull();
    expect(screen.queryByRole('checkbox', { name: /focused display/i })).toBeNull();
  });

  it('renders a searchable keyboard picker with all 38 providers enabled', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i });

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    const listbox = screen.getByRole('listbox', { name: 'Smart processing providers' });
    const options = within(listbox).getAllByRole('option');
    expect(options).toHaveLength(38);
    expect(options.filter((option) => option.hasAttribute('disabled'))).toHaveLength(0);
    expect(listbox.querySelectorAll('img')).toHaveLength(38);

    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Groq');
    const filtered = screen.getByRole('listbox', { name: 'Smart processing providers' });
    expect(within(filtered).getAllByRole('option')).toHaveLength(1);
    filtered.focus();
    await user.keyboard('{Enter}');

    await waitFor(() =>
      expect(saveConfig).toHaveBeenCalledWith({
        providerId: 'groq',
        modelId: 'llama-3.1-8b-instant',
      }),
    );
    expect(await screen.findByRole('button', { name: /Groq.*The fastest/i })).toBeVisible();
  });

  it.each(PROVIDER_CATALOG)(
    'renders the complete provider form matrix for $id',
    async (provider) => {
      const user = renderSettings();
      await user.click(screen.getByRole('button', { name: 'Settings' }));
      await user.click(screen.getByRole('button', { name: 'Smart processing' }));
      await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
      const option = document.querySelector(`#${provider.id}`);
      expect(option).toBeVisible();
      expect(option).toHaveTextContent(provider.displayName);
      expect(option).toHaveTextContent(provider.description);
      expect(option?.querySelector('img')).toBeInTheDocument();
    },
  );

  it('discovers Pi models without auto-selection and keeps explicit manual entry available', async () => {
    listModels.mockResolvedValue([
      {
        id: 'anthropic/claude-test',
        name: 'anthropic/claude-test',
        contextWindow: 200_000,
        vision: 'supported',
      },
    ]);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));

    await waitFor(() => expect(listModels).toHaveBeenCalledWith('pi', expect.any(String), false));
    expect(await screen.findByRole('combobox', { name: 'Pi model' })).toHaveValue('');
    expect(screen.getByRole('combobox', { name: 'Thinking level' })).toHaveValue('off');
    expect(screen.getByText(/minimal fixed prompt.*may contact.*charge/iu)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Enter model ID manually' }));
    const manual = screen.getByRole('textbox', { name: 'Pi model' });
    await user.type(manual, 'custom/exact-model');
    expect(manual).toHaveValue('custom/exact-model');
    expect(screen.queryByLabelText('API key', { selector: 'input' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Discover models' })).toBeEnabled(),
    );
    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    await waitFor(() =>
      expect(listModels).toHaveBeenLastCalledWith('pi', expect.any(String), true),
    );
  });

  it('validates manual and browsed Pi paths and refreshes models immediately', async () => {
    listModels.mockResolvedValue([
      { id: 'p/one', name: 'p/one', contextWindow: 8_000, vision: 'unsupported' },
    ]);
    const ready = {
      mode: 'configured' as const,
      state: 'ready' as const,
      configuredPath: 'C:\\Program Files\\npm\\pi.cmd',
      path: 'C:\\Program Files\\npm\\node_modules\\@earendil-works\\pi-coding-agent\\dist\\cli.js',
      version: '0.81.0',
      source: 'configured' as const,
      errorCode: null,
    };
    const status = vi
      .spyOn(window.talkingQuill.providers, 'piInstallationStatus')
      .mockResolvedValue({ ...ready, mode: 'automatic', configuredPath: null, source: 'path' });
    const savePath = vi
      .spyOn(window.talkingQuill.providers, 'savePiInstallation')
      .mockResolvedValue(ready);
    const browse = vi
      .spyOn(window.talkingQuill.providers, 'browsePiInstallation')
      .mockResolvedValue(ready);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));
    await waitFor(() => expect(status).toHaveBeenCalled());

    const input = screen.getByRole('textbox', { name: 'Pi installation path' });
    await user.type(input, 'C:\\Program Files\\npm\\pi.cmd');
    const before = listModels.mock.calls.length;
    await user.click(screen.getByRole('button', { name: 'Save path' }));
    await waitFor(() => expect(savePath).toHaveBeenCalledWith('C:\\Program Files\\npm\\pi.cmd'));
    await waitFor(() => expect(listModels.mock.calls.length).toBeGreaterThan(before));
    expect(await screen.findByText(/Pi 0\.81\.0/)).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Browse folder…' }));
    await waitFor(() => expect(browse).toHaveBeenCalledOnce());

    savePath.mockRejectedValueOnce({ code: 'PI_CONFIG_INVALID' });
    await user.clear(input);
    await user.type(input, 'C:\\stale\\pi.cmd');
    await user.click(screen.getByRole('button', { name: 'Save path' }));
    expect(await screen.findByText(/configured path is stale or invalid/i)).toBeVisible();
    expect(screen.queryByText(/Pi 0\.81\.0/)).toBeNull();
  });

  it('renders Pi empty and malformed discovery states with specific guidance', async () => {
    listModels.mockResolvedValueOnce([]);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));
    expect(
      await screen.findByText(/Pi returned no models.*exact saved model is retained/i),
    ).toBeVisible();
    expect(screen.getByText('No models found')).toBeVisible();

    listModels.mockRejectedValueOnce({ code: 'INVALID_RESPONSE' });
    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    expect(await screen.findByText(/model list was malformed or incompatible/i)).toBeVisible();
  });

  it('renders many Pi models without auto-selection and persists non-default thinking', async () => {
    listModels.mockResolvedValue([
      { id: 'p/one', name: 'p/one', contextWindow: 8_000, vision: 'unsupported' },
      { id: 'p/two', name: 'p/two', contextWindow: 8_000, vision: 'unsupported' },
    ]);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));
    const model = await screen.findByRole('combobox', { name: 'Pi model' });
    expect(model).toHaveValue('');
    await user.selectOptions(model, 'p/two');
    await user.selectOptions(screen.getByRole('combobox', { name: 'Thinking level' }), 'xhigh');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() =>
      expect(saveConfig).toHaveBeenLastCalledWith({
        providerId: 'pi',
        modelId: 'p/two',
        thinking: 'xhigh',
      }),
    );
    expect(settings.smartProcessing.providers.pi).toMatchObject({
      modelId: 'p/two',
      thinking: 'xhigh',
    });
  });

  it('retains a disappeared exact Pi model on refresh and permits cancellation', async () => {
    settings = settingsWithConfig(settings, {
      providerId: 'pi',
      modelId: 'p/removed',
      thinking: 'high',
    });
    listModels.mockResolvedValue([
      { id: 'p/current', name: 'p/current', contextWindow: 8_000, vision: 'unsupported' },
      { id: 'p/other', name: 'p/other', contextWindow: 8_000, vision: 'unsupported' },
    ]);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText(/exact selected Pi model.*retained/i)).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Pi model' })).toHaveValue('p/removed');
    await user.click(screen.getByRole('button', { name: 'Choose a discovered model' }));
    const selector = screen.getByRole('combobox', { name: 'Pi model' });
    expect(selector).toHaveValue('');
    await user.selectOptions(selector, 'p/current');
    expect(selector).toHaveValue('p/current');

    const pending = deferred<readonly []>();
    listModels.mockReturnValueOnce(pending.promise);
    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    await user.click(screen.getByRole('button', { name: 'Cancel discovery' }));
    expect(cancel).toHaveBeenCalledWith(expect.stringContaining('pi-models-'));
    pending.resolve([]);
    expect(await screen.findByText('Model discovery cancelled.')).toBeVisible();
  });

  it('renders validated native Azure and Bedrock configuration controls', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Azure');
    await user.click(screen.getByRole('option', { name: /^Azure OpenAI/ }));
    expect(await screen.findByRole('textbox', { name: 'Azure resource endpoint' })).toHaveValue(
      'https://my-resource.openai.azure.com',
    );
    expect(screen.getByRole('textbox', { name: 'Deployment name' })).toBeVisible();
    expect(screen.getByRole('combobox', { name: 'Model type' })).toHaveValue('default');
    expect(screen.getByLabelText('API key', { selector: 'input' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Discover models' })).not.toBeInTheDocument();
    expect(
      screen.getByText(/Deployment discovery requires Azure management-plane credentials/),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: /^Azure OpenAI/ }));
    await user.clear(screen.getByRole('searchbox', { name: 'Search providers' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Bedrock');
    await user.click(screen.getByRole('option', { name: /^AWS Bedrock/ }));
    expect(await screen.findByRole('combobox', { name: 'AWS region' })).toHaveValue('us-west-2');
    expect(screen.getByRole('textbox', { name: 'Model' })).toBeVisible();
  });

  it('stores structured Bedrock credentials write-only and clears every field immediately', async () => {
    const pending = deferred<ProviderCredentialState>();
    setSecret.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Bedrock');
    await user.click(screen.getByRole('option', { name: /^AWS Bedrock/ }));

    const accessKey = screen.getByLabelText('AWS access key ID');
    const secretKey = screen.getByLabelText('AWS secret access key');
    const sessionToken = screen.getByLabelText('AWS session token (optional)');
    await user.type(accessKey, 'AKIDCOMPONENT1234');
    await user.type(secretKey, 'component-secret-access-key');
    await user.type(sessionToken, 'component-session-token');
    await user.click(screen.getByRole('button', { name: 'Store AWS credentials' }));

    expect(setSecret).toHaveBeenCalledWith(
      'bedrock',
      BINDING_TOKEN,
      JSON.stringify({
        accessKeyId: 'AKIDCOMPONENT1234',
        secretAccessKey: 'component-secret-access-key',
        sessionToken: 'component-session-token',
      }),
    );
    expect(accessKey).toHaveValue('');
    expect(secretKey).toHaveValue('');
    expect(sessionToken).toHaveValue('');
    pending.resolve({
      providerId: 'bedrock',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    expect(await screen.findByText('Configured')).toBeVisible();
  });

  it('requires an explicit saved Ollama model before connection testing', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Select and save a model before testing')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Test connection' })).toBeDisabled();
    expect(testConnection).not.toHaveBeenCalled();
  });

  it('clears write-only secrets immediately and supports configured, replace, and delete status', async () => {
    const pending = deferred<Awaited<ReturnType<MainApi['providers']['setSecret']>>>();
    setSecret.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    const password = await screen.findByLabelText('API key', { selector: 'input' });
    const canary = 'component-secret-canary';
    await user.type(password, canary);

    await user.click(screen.getByRole('button', { name: 'Store API key' }));

    expect(setSecret).toHaveBeenCalledWith('ollama', BINDING_TOKEN, canary);
    expect(password).toHaveValue('');
    expect(document.body.textContent).not.toContain(canary);
    pending.resolve({
      providerId: 'ollama',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    expect(await screen.findByText('Configured')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Replace API key' })).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Delete API key' }));
    await waitFor(() => expect(deleteSecret).toHaveBeenCalledWith('ollama', BINDING_TOKEN));
    expect(await screen.findByText('Not configured')).toBeVisible();
  });

  it('discovers and selects a model, saves drafts, and exposes safe connection retry states', async () => {
    const hostileMessage = 'https://secret.invalid bearer should-not-render';
    testConnection.mockRejectedValueOnce(
      Object.assign(new Error(hostileMessage), { code: 'AUTHENTICATION_FAILED' }),
    );
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Discover models' });

    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    expect(await screen.findByText('1 models found')).toBeVisible();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Model' }), 'llama3.2');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));

    await waitFor(() =>
      expect(saveConfig).toHaveBeenLastCalledWith(
        expect.objectContaining({ providerId: 'ollama', modelId: 'llama3.2' }),
      ),
    );
    expect(await screen.findByText('Draft saved')).toBeVisible();
    expect(screen.getByText(/Local destination — verified/)).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(await screen.findByText(/rejected the API key/i)).toBeVisible();
    expect(document.body.textContent).not.toContain(hostileMessage);
    await user.click(screen.getByRole('button', { name: 'Retry test' }));
    expect(
      await screen.findByText(/Connection verified. 1 compatible model reported/),
    ).toBeVisible();
  });

  it('ignores a stale connection result after switching providers', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pendingTest = deferred<Awaited<ReturnType<MainApi['providers']['testConnection']>>>();
    testConnection.mockReturnValueOnce(pendingTest.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Test connection' });
    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    const oldOperationId = testConnection.mock.calls[0]?.[1];

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));
    expect(await screen.findByText(/Cloud destination — not yet verified/)).toBeVisible();
    expect(cancel).toHaveBeenCalledWith(oldOperationId);

    pendingTest.resolve({ ok: true, destination: 'local', modelCount: 1 });
    await waitFor(() =>
      expect(screen.getByText(/Cloud destination — not yet verified/)).toBeVisible(),
    );
    expect(screen.queryByText(/Connection verified/)).not.toBeInTheDocument();
  });

  it('invalidates endpoint-bound credentials and requires visible re-entry before endpoint B use', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    secretStatus.mockImplementation((providerId) => {
      const configured =
        settings.smartProcessing.providers.ollama?.baseUrl !== 'http://127.0.0.1:22334';
      return Promise.resolve({
        providerId,
        configured,
        updatedAt: configured ? 1 : null,
        bindingToken: BINDING_TOKEN,
      });
    });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Configured')).toBeVisible();

    const endpoint = screen.getByRole('textbox', { name: 'Endpoint URL' });
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:22334');

    expect(screen.getByText(/Provider destination changed.*re-enter credentials/i)).toBeVisible();
    expect(screen.getByRole('button', { name: 'Store API key' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Discover models' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Test connection' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() =>
      expect(saveConfig).toHaveBeenLastCalledWith(
        expect.objectContaining({ providerId: 'ollama', baseUrl: 'http://127.0.0.1:22334' }),
      ),
    );
    expect(await screen.findByText('Not configured')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Store API key' })).toBeEnabled();
  });

  it('never exposes the previous binding status while the new status is pending', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const oldBindingStatus = deferred<ProviderCredentialState>();
    const newBindingStatus = deferred<ProviderCredentialState>();
    secretStatus
      .mockReturnValueOnce(oldBindingStatus.promise)
      .mockReturnValue(newBindingStatus.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));

    const endpoint = await screen.findByRole('textbox', { name: 'Endpoint URL' });
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:42334');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(secretStatus.mock.calls.length).toBeGreaterThanOrEqual(2));

    expect(screen.getByText('Not configured')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Store API key' })).toBeDisabled();
    expect(screen.queryByRole('button', { name: 'Replace API key' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Delete API key' })).not.toBeInTheDocument();

    oldBindingStatus.resolve({
      providerId: 'ollama',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    await waitFor(() => expect(screen.getByText('Not configured')).toBeVisible());
    expect(screen.queryByRole('button', { name: 'Delete API key' })).not.toBeInTheDocument();

    newBindingStatus.resolve({
      providerId: 'ollama',
      configured: false,
      updatedAt: null,
      bindingToken: BINDING_TOKEN,
    });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Store API key' })).toBeEnabled(),
    );
    expect(screen.getByText('Not configured')).toBeVisible();
  });

  it('refreshes the committed binding status when post-commit cleanup reports failure', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    let configured = true;
    secretStatus.mockImplementation((providerId) =>
      Promise.resolve({
        providerId,
        configured,
        updatedAt: configured ? 1 : null,
        bindingToken: BINDING_TOKEN,
      }),
    );
    saveConfig.mockImplementationOnce((config) => {
      settings = settingsWithConfig(settings, config);
      configured = false;
      settingsListener?.(settings);
      return Promise.reject(Object.assign(new Error('cleanup failed'), { code: 'UNAVAILABLE' }));
    });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Configured')).toBeVisible();

    const endpoint = screen.getByRole('textbox', { name: 'Endpoint URL' });
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:32334');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));

    expect(await screen.findByText('Configuration update failed; status refreshed')).toBeVisible();
    expect(await screen.findByText('Not configured')).toBeVisible();
    expect(secretStatus.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('cancels stale destination checks and ignores their late privacy result', async () => {
    const firstDestination = deferred<'local'>();
    const secondDestination = deferred<'cloud'>();
    destination
      .mockReturnValueOnce(firstDestination.promise)
      .mockReturnValueOnce(secondDestination.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    const password = await screen.findByLabelText('API key', { selector: 'input' });
    await user.type(password, 'destination-secret');
    await user.click(screen.getByRole('button', { name: 'Store API key' }));
    await waitFor(() => expect(destination).toHaveBeenCalledTimes(1));
    const firstOperationId = destination.mock.calls[0]?.[1];

    await user.type(screen.getByRole('textbox', { name: 'Model' }), 'manual-model');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(destination).toHaveBeenCalledTimes(2));
    const secondOperationId = destination.mock.calls[1]?.[1];

    expect(cancel).toHaveBeenCalledWith(firstOperationId);
    expect(firstOperationId).not.toBe(secondOperationId);
    secondDestination.resolve('cloud');
    expect(await screen.findByText(/Cloud destination — verified/)).toBeVisible();
    firstDestination.resolve('local');
    await waitFor(() => expect(screen.getByText(/Cloud destination — verified/)).toBeVisible());
    expect(screen.getByText(/provider may charge your account/i)).toBeVisible();
  });

  it('keeps overlapping model and connection cancellation scoped to the right operation', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pendingModels = deferred<readonly []>();
    const pendingTest = deferred<Awaited<ReturnType<MainApi['providers']['testConnection']>>>();
    listModels.mockReturnValueOnce(pendingModels.promise);
    testConnection.mockReturnValueOnce(pendingTest.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Discover models' });

    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    const modelOperationId = listModels.mock.calls[0]?.[1];
    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(screen.getByText(/Testing selected model… 0\.0s/u)).toBeVisible();
    const connectionOperationId = testConnection.mock.calls[0]?.[1];
    await user.click(screen.getByRole('button', { name: 'Cancel discovery' }));
    await user.click(screen.getByRole('button', { name: 'Cancel test' }));

    expect(modelOperationId).toBeTruthy();
    expect(connectionOperationId).toBeTruthy();
    expect(modelOperationId).not.toBe(connectionOperationId);
    expect(cancel).toHaveBeenNthCalledWith(1, modelOperationId);
    expect(cancel).toHaveBeenNthCalledWith(2, connectionOperationId);
    pendingModels.resolve([]);
    pendingTest.resolve({ ok: true, destination: 'local', modelCount: 0 });
  });

  it('shows loading cancellation and cloud cost qualification', async () => {
    const pending = deferred<readonly []>();
    listModels.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Discover models' });
    await user.click(screen.getByRole('button', { name: 'Discover models' }));
    await user.click(screen.getByRole('button', { name: 'Cancel discovery' }));
    expect(cancel).toHaveBeenCalledOnce();
    pending.resolve([]);
    expect(await screen.findByText('Model discovery cancelled.')).toBeVisible();
    expect(screen.queryByText('No models found')).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    const openai = screen.getByRole('option', { name: /^OpenAI.*standard option/ });
    await user.click(openai);
    expect(await screen.findByText(/provider may charge your account/i)).toBeVisible();
    expect(screen.getByText(/Cloud destination — not yet verified/)).toBeVisible();
  });
});

function settingsWithConfig(current: Settings, config: ProviderConfig): Settings {
  const { providerId, ...draft } = config;
  return {
    ...current,
    smartProcessing: {
      ...current.smartProcessing,
      selectedProviderId: providerId,
      providers: { ...current.smartProcessing.providers, [providerId]: draft },
    },
  };
}

function deferred<Value>() {
  let resolvePromise!: (value: Value | PromiseLike<Value>) => void;
  let rejectPromise!: (reason?: unknown) => void;
  const promise = new Promise<Value>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, resolve: resolvePromise, reject: rejectPromise };
}
