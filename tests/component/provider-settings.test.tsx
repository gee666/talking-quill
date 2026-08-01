// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { PROVIDER_CATALOG } from '../../app/src/main/providers/registry';
import { AppShell } from '../../app/src/renderer/main/AppShell';
import { reconcileDiscoveredModels } from '../../app/src/renderer/main/pi-model-selection';
import { resetAutoDiscoveryMemory } from '../../app/src/renderer/main/smart-processing/auto-discovery-memory';
import { CUSTOM_MODEL_OPTION } from '../../app/src/renderer/main/smart-processing/ProviderFieldControl';
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
const LEGACY_PROVIDER_ENDPOINTS = [
  'file:///legacy/provider',
  'ftp://legacy.example/models',
] as const;

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
        lastStep: 5,
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
    updateState: () =>
      Promise.resolve({
        phase: 'idle',
        currentVersion: '1.0.0',
        availableVersion: null,
        releaseUrl: null,
        percent: null,
        message: null,
        revision: 0,
      }),
    applyUpdate: () =>
      Promise.resolve({
        phase: 'downloading',
        currentVersion: '1.0.0',
        availableVersion: '1.1.0',
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
        percent: 0,
        message: null,
        revision: 1,
      }),
    onUpdateChanged: () => () => undefined,
    openPermissionSettings: () => Promise.resolve(),
    openLocation: () => Promise.resolve(),
    openRelease: () => Promise.resolve(),
    notices: () => Promise.resolve('Third-party notices'),
  },
  shortcutCapture: {
    start: () => Promise.resolve(),
    stop: () => Promise.resolve(),
  },
  activationTest: {
    start: () =>
      Promise.resolve({
        active: true,
        phase: 'waiting',
        profileId: null,
        shortcut: null,
        elapsedMs: 0,
        unavailableReason: null,
      }),
    stop: () =>
      Promise.resolve({
        active: false,
        phase: 'idle',
        profileId: null,
        shortcut: null,
        elapsedMs: 0,
        unavailableReason: null,
      }),
    onChanged: () => () => undefined,
  },
  app: {
    getBootstrap: () =>
      Promise.resolve({
        appVersion: '1.0.0',
        platform: 'win32',
        state: { enabled: true, status: 'needs-setup', modelReady: false, helper },
        settings,
      }),
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
          defaultProcessingMode: settings.app.defaultProcessingMode,
          widgetSize: patch.app?.widgetSize ?? settings.app.widgetSize,
          soundsEnabled: patch.app?.soundsEnabled ?? settings.app.soundsEnabled,
          launchAtLogin: patch.app?.launchAtLogin ?? settings.app.launchAtLogin,
        },
        recording: {
          preferredMicrophoneId:
            patch.recording?.preferredMicrophoneId ?? settings.recording.preferredMicrophoneId,
          silencePreset: patch.recording?.silencePreset ?? settings.recording.silencePreset,
          autoSubmitOnSilence:
            patch.recording?.autoSubmitOnSilence ?? settings.recording.autoSubmitOnSilence,
          includeSystemAudio:
            patch.recording?.includeSystemAudio ?? settings.recording.includeSystemAudio,
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
    browsePiInstallation: () => Promise.resolve(null),
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
  // Automatic discovery deliberately remembers attempts beyond a component instance; each test
  // starts from a fresh application session.
  resetAutoDiscoveryMemory();
  settings = structuredClone(DEFAULT_SETTINGS);
  settings.welcome = {
    completedAt: 1,
    lastStep: 5,
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
  osaStatus.mockImplementation(() => {
    const providerId = settings.smartProcessing.selectedProviderId;
    return Promise.resolve({
      providerId,
      modelId: settings.smartProcessing.providers[providerId]?.modelId ?? null,
      capability: 'supported',
      manualTestAllowed: false,
      screenPermission: 'granted',
    });
  });
  setOnScreenAwareness.mockReset();
  setOnScreenAwareness.mockResolvedValue(structuredClone(DEFAULT_SETTINGS));
  verifyVision.mockReset();
  verifyVision.mockResolvedValue({ verificationId: '11111111-1111-4111-8111-111111111112' });
  confirmVision.mockReset();
  confirmVision.mockResolvedValue(structuredClone(DEFAULT_SETTINGS));
  Object.defineProperty(window, 'talkingQuill', { configurable: true, value: api });
});

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
});

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
    expect(reconcileDiscoveredModels([first, second], { modelId: 'p/two' }, true).message).toBe(
      null,
    );
    expect(reconcileDiscoveredModels([first], { modelId: 'p/two' }, true).message).toMatch(
      /exact selected Pi model.*retained/u,
    );
    expect(reconcileDiscoveredModels([first], {}, true).message).toBeNull();
    expect(reconcileDiscoveredModels([first, second], {}, true).message).toBeNull();
    expect(reconcileDiscoveredModels([], { modelId: 'p/two' }, true).message).toMatch(
      /exact saved model is retained/u,
    );
  });
});

describe('Smart processing settings', () => {
  it('keeps unknown vision off and cancels disclosed live image-echo verification', async () => {
    settings = settingsWithConfig(settings, {
      providerId: 'generic-openai',
      baseUrl: 'https://example.test/v1',
      modelId: 'private-model',
    });
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(
      await screen.findByText(
        'We cannot tell whether this model can see pictures, so this stays off.',
      ),
    ).toBeVisible();
    expect(screen.queryByRole('checkbox', { name: /see your screen/i })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Run a quick screen test' }));
    expect(
      screen.getByRole('dialog', { name: 'Check that the AI can see your screen' }),
    ).toHaveTextContent('send that one picture to the AI service. Nothing is kept.');
    await user.click(screen.getByRole('button', { name: 'Capture and check' }));
    await user.click(screen.getByRole('button', { name: /^Cancel$/ }));
    expect(cancel).toHaveBeenCalledWith(expect.stringContaining('-vision-'));
    if (pendingVision.resolve === null) throw new Error('Vision verification was not pending');
    pendingVision.resolve({ verificationId: '11111111-1111-4111-8111-111111111113' });
    await waitFor(() => expect(verifyVision).toHaveBeenCalledOnce());
    expect(confirmVision).not.toHaveBeenCalled();
    expect(
      screen.queryByRole('dialog', { name: 'Check that the AI can see your screen' }),
    ).toBeNull();
    expect(screen.queryByRole('checkbox', { name: /see your screen/i })).toBeNull();
  });

  it('renders a searchable keyboard picker with all 38 providers enabled', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
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

  it.each(LEGACY_PROVIDER_ENDPOINTS)(
    'opens an unselected inert legacy endpoint for safe repair without executing it: %s',
    async (baseUrl) => {
      settings.smartProcessing.providers['generic-openai'] = {
        baseUrl,
        modelId: 'legacy-model',
      };
      const user = renderSettings();
      await user.click(screen.getByRole('button', { name: 'Settings' }));
      await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
      await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
      await user.type(
        screen.getByRole('searchbox', { name: 'Search providers' }),
        'Generic OpenAI',
      );
      await user.click(screen.getByRole('option', { name: /^Generic OpenAI.*Connect/ }));

      const endpoint = await screen.findByRole('textbox', { name: 'Endpoint URL' });
      expect(endpoint).toHaveValue(baseUrl);
      expect(screen.getByText(/stored legacy endpoint is inactive/i)).toBeVisible();
      expect(
        screen.getByText(/We do not know yet where your text goes — not checked yet/i),
      ).toBeVisible();
      expect(screen.getByRole('button', { name: 'Refresh list' })).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Test connection' })).toBeDisabled();
      expect(screen.getByRole('button', { name: 'Save configuration' })).toBeDisabled();
      expect(saveConfig).not.toHaveBeenCalled();
      expect(testConnection).not.toHaveBeenCalled();
      // The initially selected Ollama provider legitimately runs one automatic discovery, but the
      // provider awaiting endpoint repair must never be contacted.
      expect(callsFor(listModels, 'generic-openai')).toEqual([]);
      expect(callsFor(destination, 'generic-openai')).toEqual([]);

      await user.clear(endpoint);
      await user.type(endpoint, 'ftp://still-inert.example/models');
      expect(screen.getByRole('button', { name: 'Save configuration' })).toBeDisabled();
      await user.clear(endpoint);
      await user.type(endpoint, 'https://repaired.example.test/v1');
      expect(screen.queryByText(/stored legacy endpoint is inactive/i)).toBeNull();
      await user.click(screen.getByRole('button', { name: 'Save configuration' }));

      await waitFor(() =>
        expect(saveConfig).toHaveBeenCalledWith(
          expect.objectContaining({
            providerId: 'generic-openai',
            baseUrl: 'https://repaired.example.test/v1',
            modelId: 'legacy-model',
          }),
        ),
      );
      expect(await screen.findByText('Saved')).toBeVisible();
      expect(await screen.findByRole('button', { name: /^Generic OpenAI.*Connect/ })).toBeVisible();
    },
  );

  it('keeps the authoritative baseline across chained legacy repair selections', async () => {
    settings.smartProcessing.providers['generic-openai'] = {
      baseUrl: 'file:///legacy/provider',
      modelId: 'legacy-model',
    };
    settings.smartProcessing.providers.litellm = {
      baseUrl: 'ftp://legacy.example/models',
      modelId: 'legacy-model',
    };
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Generic OpenAI');
    await user.click(screen.getByRole('option', { name: /^Generic OpenAI.*Connect/ }));
    await user.click(await screen.findByRole('button', { name: /^Generic OpenAI.*Connect/ }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'LiteLLM');
    await user.click(screen.getByRole('option', { name: /^LiteLLM/ }));
    expect(await screen.findByRole('textbox', { name: 'Endpoint URL' })).toHaveValue(
      'ftp://legacy.example/models',
    );

    settings = {
      ...settings,
      app: { ...settings.app, soundsEnabled: !settings.app.soundsEnabled },
    };
    act(() => {
      settingsListener?.(settings);
    });

    expect(await screen.findByRole('button', { name: /^LiteLLM/ })).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Endpoint URL' })).toHaveValue(
      'ftp://legacy.example/models',
    );
    expect(saveConfig).not.toHaveBeenCalled();
  });

  it('abandons a repair draft when the authoritative provider changes', async () => {
    settings.smartProcessing.providers['generic-openai'] = {
      baseUrl: 'file:///legacy/provider',
      modelId: 'legacy-model',
    };
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Generic OpenAI');
    await user.click(screen.getByRole('option', { name: /^Generic OpenAI.*Connect/ }));
    expect(await screen.findByText(/stored legacy endpoint is inactive/i)).toBeVisible();

    settings = settingsWithConfig(settings, {
      providerId: 'openai',
      modelId: 'gpt-4.1-mini',
    });
    act(() => {
      settingsListener?.(settings);
    });

    expect(await screen.findByRole('button', { name: /^OpenAI.*standard option/ })).toBeVisible();
    expect(saveConfig).not.toHaveBeenCalled();
  });

  it('keeps the persisted provider active while a new selection is saving or rejected', async () => {
    const pending = deferred<Awaited<ReturnType<MainApi['providers']['saveConfig']>>>();
    saveConfig.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    const osa = await screen.findByRole('checkbox', {
      name: 'Let the AI see your screen',
    });
    expect(osa).toBeEnabled();

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));

    expect(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i })).toBeDisabled();
    expect(osa).toBeDisabled();
    await user.click(osa);
    expect(setOnScreenAwareness).not.toHaveBeenCalled();

    pending.reject(new Error('selection write failed'));
    expect(
      await screen.findByText('That did not save. Check the settings and try again.'),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i })).toBeEnabled();
  });

  it('adopts a provider selection that commits before post-commit cleanup rejects', async () => {
    saveConfig.mockImplementationOnce((config) => {
      settings = settingsWithConfig(settings, config);
      settingsListener?.(settings);
      return Promise.reject(new Error('post-commit cleanup failed'));
    });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));

    expect(await screen.findByRole('button', { name: /^OpenAI.*standard option/ })).toBeVisible();
    expect(
      await screen.findByText('That did not save. Check the settings and try again.'),
    ).toBeVisible();
  });

  it('reconciles an authoritative provider event received while mounted', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i });

    settings = settingsWithConfig(settings, {
      providerId: 'openai',
      modelId: 'gpt-4.1-mini',
    });
    act(() => {
      settingsListener?.(settings);
    });

    expect(await screen.findByRole('button', { name: /^OpenAI.*standard option/ })).toBeVisible();
  });

  it('refreshes retained credentials when a rejected selection invalidates an in-flight status request', async () => {
    const staleStatus = deferred<ProviderCredentialState>();
    const refreshedStatus = deferred<ProviderCredentialState>();
    secretStatus
      .mockReturnValueOnce(staleStatus.promise)
      .mockReturnValueOnce(refreshedStatus.promise);
    const selection = deferred<Awaited<ReturnType<MainApi['providers']['saveConfig']>>>();
    saveConfig.mockReturnValueOnce(selection.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await waitFor(() => expect(secretStatus).toHaveBeenCalledOnce());

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));
    selection.reject(new Error('selection write failed'));

    await waitFor(() => expect(secretStatus).toHaveBeenCalledTimes(2));
    staleStatus.resolve({
      providerId: 'ollama',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    refreshedStatus.resolve({
      providerId: 'ollama',
      configured: false,
      updatedAt: null,
      bindingToken: BINDING_TOKEN,
    });

    expect(
      await screen.findByText('That did not save. Check the settings and try again.'),
    ).toBeVisible();
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Store API key' })).toBeEnabled(),
    );
    expect(screen.queryByRole('button', { name: 'Replace API key' })).toBeNull();
    expect(screen.queryByRole('button', { name: 'Delete API key' })).toBeNull();
  });

  it('blocks credential mutations until a provider selection settles and restores the prior provider after rejection', async () => {
    const pending = deferred<Awaited<ReturnType<MainApi['providers']['saveConfig']>>>();
    saveConfig.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    const password = await screen.findByLabelText('API key', { selector: 'input' });
    const storeCredential = screen.getByRole('button', { name: 'Store API key' });
    await waitFor(() => expect(storeCredential).toBeEnabled());
    await user.type(password, 'selection-race-secret');

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));

    expect(password).toBeDisabled();
    expect(storeCredential).toBeDisabled();
    await user.click(storeCredential);
    expect(setSecret).not.toHaveBeenCalled();
    expect(saveConfig).toHaveBeenCalledTimes(1);

    pending.reject(new Error('selection write failed'));
    expect(
      await screen.findByText('That did not save. Check the settings and try again.'),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i })).toBeEnabled();
    expect(password).toBeEnabled();
    expect(storeCredential).toBeEnabled();
    expect(password).toHaveValue('selection-race-secret');
  });

  it('clears unsaved credentials after a successful provider switch', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    const ollamaCredential = await screen.findByLabelText('API key', { selector: 'input' });
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Store API key' })).toBeEnabled(),
    );
    await user.type(ollamaCredential, 'must-not-cross-providers');

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));

    expect(await screen.findByRole('button', { name: /^OpenAI.*standard option/ })).toBeVisible();
    expect(screen.getByLabelText('API key', { selector: 'input' })).toHaveValue('');
  });

  it.each(PROVIDER_CATALOG)(
    'renders the complete provider form matrix for $id',
    async (provider) => {
      const user = renderSettings();
      await user.click(screen.getByRole('button', { name: 'Settings' }));
      await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));

    await waitFor(() => expect(listModels).toHaveBeenCalledWith('pi', expect.any(String), false));
    expect(await screen.findByRole('combobox', { name: 'Pi model' })).toHaveValue('');
    expect(screen.getByRole('combobox', { name: 'Thinking level' })).toHaveValue('off');
    expect(
      screen.getByText(/one tiny message to the model you picked.*fraction of a cent/i),
    ).toBeVisible();
    await user.selectOptions(
      screen.getByRole('combobox', { name: 'Pi model' }),
      CUSTOM_MODEL_OPTION,
    );
    const manual = screen.getByRole('textbox', { name: 'Pi model name' });
    await user.type(manual, 'custom/exact-model');
    expect(manual).toHaveValue('custom/exact-model');
    expect(screen.queryByLabelText('API key', { selector: 'input' })).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(screen.getByRole('button', { name: 'Refresh list' })).toBeEnabled());
    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
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
      .mockResolvedValue('C:\\Browsed\\pi.cmd');
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
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
    await waitFor(() => expect(savePath).toHaveBeenLastCalledWith('C:\\Browsed\\pi.cmd'));

    savePath.mockRejectedValueOnce({ code: 'PI_CONFIG_INVALID' });
    await user.clear(input);
    await user.type(input, 'C:\\stale\\pi.cmd');
    await user.click(screen.getByRole('button', { name: 'Save path' }));
    expect(await screen.findByText(/Nothing usable is at that path any more/i)).toBeVisible();
    expect(screen.queryByText(/Pi 0\.81\.0/)).toBeNull();
  });

  it('keeps Pi dialog dwell separate from bounded validation and does not persist cancellation', async () => {
    const dialog = deferred<string | null>();
    const browse = vi
      .spyOn(window.talkingQuill.providers, 'browsePiInstallation')
      .mockReturnValueOnce(dialog.promise);
    const savePath = vi.spyOn(window.talkingQuill.providers, 'savePiInstallation');
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));

    const browseButton = await screen.findByRole('button', { name: 'Browse folder…' });
    await user.click(browseButton);
    expect(browse).toHaveBeenCalledOnce();
    expect(browseButton).toBeDisabled();
    expect(savePath).not.toHaveBeenCalled();

    dialog.resolve(null);
    await waitFor(() => expect(browseButton).toBeEnabled());
    expect(savePath).not.toHaveBeenCalled();
  });

  it('prevents Pi path and provider configuration mutations from overlapping', async () => {
    const pendingPath = deferred<Awaited<ReturnType<MainApi['providers']['savePiInstallation']>>>();
    vi.spyOn(window.talkingQuill.providers, 'savePiInstallation').mockReturnValueOnce(
      pendingPath.promise,
    );
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));

    const path = await screen.findByRole('textbox', { name: 'Pi installation path' });
    await user.type(path, 'C:\\Program Files\\npm\\pi.cmd');
    await user.click(screen.getByRole('button', { name: 'Save path' }));

    expect(screen.getByRole('button', { name: /^PiUse/ })).toBeDisabled();
    expect(screen.getByRole('combobox', { name: 'Pi model' })).toBeDisabled();
    pendingPath.resolve({
      mode: 'configured',
      state: 'ready',
      configuredPath: 'C:\\Program Files\\npm\\pi.cmd',
      path: 'C:\\Program Files\\npm\\node_modules\\pi\\dist\\cli.js',
      version: '0.81.0',
      source: 'configured',
      errorCode: null,
    });
    await waitFor(() => expect(path).toBeEnabled());
  });

  it('renders Pi empty and malformed discovery states with specific guidance', async () => {
    // Automatic discovery already runs for the initially selected provider, so the empty result
    // has to be bound to Pi rather than queued positionally.
    listModels.mockImplementation((providerId) =>
      Promise.resolve(
        providerId === 'pi'
          ? []
          : [{ id: 'llama3.2', name: 'Llama 3.2', contextWindow: 8_192, vision: 'unsupported' }],
      ),
    );
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Pi');
    await user.click(screen.getByRole('option', { name: /^PiUse/ }));
    expect(
      await screen.findByText(/Pi returned no models.*exact saved model is retained/i),
    ).toBeVisible();
    expect(screen.getByText('No models found — you can type a name instead.')).toBeVisible();

    listModels.mockRejectedValueOnce({ code: 'INVALID_RESPONSE' });
    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    expect(await screen.findByText(/model list was malformed or incompatible/i)).toBeVisible();
  });

  it('renders many Pi models without auto-selection and persists non-default thinking', async () => {
    listModels.mockResolvedValue([
      { id: 'p/one', name: 'p/one', contextWindow: 8_000, vision: 'unsupported' },
      { id: 'p/two', name: 'p/two', contextWindow: 8_000, vision: 'unsupported' },
    ]);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText(/exact selected Pi model.*retained/i)).toBeVisible();
    expect(screen.getByRole('textbox', { name: 'Pi model name' })).toHaveValue('p/removed');
    const selector = screen.getByRole('combobox', { name: 'Pi model' });
    expect(selector).toHaveValue(CUSTOM_MODEL_OPTION);
    await user.selectOptions(selector, 'p/current');
    expect(screen.queryByRole('textbox', { name: 'Pi model name' })).toBeNull();
    expect(selector).toHaveValue('p/current');

    const pending = deferred<readonly []>();
    listModels.mockReturnValueOnce(pending.promise);
    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    await user.click(screen.getByRole('button', { name: 'Stop searching' }));
    expect(cancel).toHaveBeenCalledWith(expect.stringContaining('pi-models-'));
    pending.resolve([]);
    expect(await screen.findByText('Model discovery cancelled.')).toBeVisible();
  });

  it('tests Text Generation WebUI using its currently loaded model without model controls', async () => {
    settings = settingsWithConfig(settings, {
      providerId: 'textgenwebui',
      baseUrl: 'http://127.0.0.1:5000/v1',
      contextWindow: 4_096,
    });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));

    expect(await screen.findByText(/model already loaded.*no model ID is required/i)).toBeVisible();
    expect(screen.queryByRole('textbox', { name: 'Model name' })).not.toBeInTheDocument();
    expect(screen.queryByRole('combobox', { name: 'Model' })).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Refresh list' })).not.toBeInTheDocument();
    expect(screen.queryByText('Pick a model and save before testing')).not.toBeInTheDocument();
    expect(screen.getByText(/whichever model it has loaded/i)).toBeVisible();
    const testButton = screen.getByRole('button', { name: 'Test connection' });
    expect(testButton).toBeEnabled();
    await user.click(testButton);
    expect(testConnection).toHaveBeenCalledWith(
      'textgenwebui',
      expect.stringContaining('textgenwebui-test-'),
    );
  });

  it('renders validated native Azure and Bedrock configuration controls', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Azure');
    await user.click(screen.getByRole('option', { name: /^Azure OpenAI/ }));
    expect(await screen.findByRole('textbox', { name: 'Azure resource endpoint' })).toHaveValue(
      'https://my-resource.openai.azure.com',
    );
    expect(screen.getByRole('textbox', { name: 'Deployment name' })).toBeVisible();
    expect(screen.getByRole('combobox', { name: 'Model type' })).toHaveValue('default');
    expect(screen.getByLabelText('API key', { selector: 'input' })).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Refresh list' })).not.toBeInTheDocument();
    expect(
      screen.getByText(/Azure keeps its deployment list behind a separate management login/),
    ).toBeVisible();

    await user.click(screen.getByRole('button', { name: /^Azure OpenAI/ }));
    await user.clear(screen.getByRole('searchbox', { name: 'Search providers' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Bedrock');
    await user.click(screen.getByRole('option', { name: /^AWS Bedrock/ }));
    expect(await screen.findByRole('textbox', { name: 'AWS region' })).toHaveValue('us-west-2');
    expect(screen.getByRole('combobox', { name: 'Model' })).toBeVisible();
  });

  it('renders provider-managed Text Generation WebUI guidance without Azure deployment advice', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Oobabooga');
    await user.click(screen.getByRole('option', { name: /^Oobabooga Web UI/ }));

    expect(
      await screen.findByText(/uses the model it already has loaded.*no model to choose/i),
    ).toBeVisible();
    expect(
      screen.queryByText(/Azure keeps its deployment list behind a separate management login/),
    ).toBeNull();
    expect(screen.queryByRole('button', { name: 'Refresh list' })).toBeNull();
    expect(screen.queryByRole('textbox', { name: 'Model name' })).toBeNull();
    expect(screen.queryByRole('combobox', { name: 'Model' })).toBeNull();
  });

  it('stores structured Bedrock credentials write-only and clears every field immediately', async () => {
    const pending = deferred<ProviderCredentialState>();
    setSecret.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'Bedrock');
    await user.click(screen.getByRole('option', { name: /^AWS Bedrock/ }));

    const accessKey = screen.getByLabelText('AWS access key ID');
    const secretKey = screen.getByLabelText('AWS secret access key');
    const sessionToken = screen.getByLabelText('AWS session token (optional)');
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Store AWS credentials' })).toBeEnabled(),
    );
    await user.type(accessKey, 'AKIDCOMPONENT1234');
    await user.type(secretKey, 'component-secret-access-key');
    await user.type(sessionToken, 'component-session-token');
    await user.click(screen.getByRole('button', { name: 'Store AWS credentials' }));

    await waitFor(() =>
      expect(setSecret).toHaveBeenCalledWith(
        'bedrock',
        BINDING_TOKEN,
        JSON.stringify({
          accessKeyId: 'AKIDCOMPONENT1234',
          secretAccessKey: 'component-secret-access-key',
          sessionToken: 'component-session-token',
        }),
      ),
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Pick a model and save before testing')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Test connection' })).toBeDisabled();
    expect(testConnection).not.toHaveBeenCalled();
  });

  it('clears write-only secrets immediately and supports configured, replace, and delete status', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pending = deferred<Awaited<ReturnType<MainApi['providers']['setSecret']>>>();
    setSecret.mockReturnValueOnce(pending.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    const password = await screen.findByLabelText('API key', { selector: 'input' });
    const connectionTest = screen.getByRole('button', { name: 'Test connection' });
    const osa = await screen.findByRole('checkbox', {
      name: 'Let the AI see your screen',
    });
    expect(connectionTest).toBeEnabled();
    expect(osa).toBeEnabled();
    const canary = 'component-secret-canary';
    await user.type(password, canary);

    await user.click(screen.getByRole('button', { name: 'Store API key' }));

    expect(setSecret).toHaveBeenCalledWith('ollama', BINDING_TOKEN, canary);
    expect(password).toHaveValue('');
    expect(document.body.textContent).not.toContain(canary);
    expect(connectionTest).toBeDisabled();
    expect(osa).toBeDisabled();
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Refresh list' });

    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    expect(await screen.findByText('1 model found')).toBeVisible();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Model' }), 'llama3.2');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));

    await waitFor(() =>
      expect(saveConfig).toHaveBeenLastCalledWith(
        expect.objectContaining({ providerId: 'ollama', modelId: 'llama3.2' }),
      ),
    );
    expect(await screen.findByText('Saved')).toBeVisible();
    expect(screen.getByText(/Runs on this computer — checked/)).toBeVisible();

    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(await screen.findByText(/rejected the API key/i)).toBeVisible();
    expect(document.body.textContent).not.toContain(hostileMessage);
    await user.click(screen.getByRole('button', { name: 'Retry test' }));
    expect(
      await screen.findByText(/Connection verified. 1 compatible model reported/),
    ).toBeVisible();
  });

  it('bounds large model pickers and keeps every model searchable', async () => {
    listModels.mockResolvedValue(
      Array.from({ length: 350 }, (_, index) => ({
        id: `model-${String(index)}`,
        name: `Model ${String(index)}`,
        contextWindow: 8_192,
        vision: 'unsupported' as const,
      })),
    );
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await user.click(await screen.findByRole('button', { name: 'Refresh list' }));

    const model = await screen.findByRole('combobox', { name: 'Model' });
    // Placeholder + 200 bounded models + the "type a model name myself" sentinel.
    expect(within(model).getAllByRole('option')).toHaveLength(202);
    const searchModels = screen.getByRole('searchbox', { name: 'Search models' });
    await user.type(searchModels, 'Model 349');
    expect(within(model).getByRole('option', { name: 'Model 349' })).toBeVisible();
    await user.selectOptions(model, 'model-349');
    expect(model).toHaveValue('model-349');
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Test connection' });
    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    const oldOperationId = testConnection.mock.calls[0]?.[1];

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    await user.click(screen.getByRole('option', { name: /^OpenAI.*standard option/ }));
    expect(await screen.findByText(/Sends your text to OpenAI — not checked yet/)).toBeVisible();
    expect(cancel).toHaveBeenCalledWith(oldOperationId);
    // OpenAI needs an API key that is not stored, so nothing is sent there without a click.
    expect(callsFor(listModels, 'openai')).toEqual([]);
    expect(callsFor(destination, 'openai')).toEqual([]);

    pendingTest.resolve({ ok: true, destination: 'local', modelCount: 1 });
    await waitFor(() =>
      expect(screen.getByText(/Sends your text to OpenAI — not checked yet/)).toBeVisible(),
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Configured')).toBeVisible();

    const endpoint = screen.getByRole('textbox', { name: 'Endpoint URL' });
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:22334');

    expect(
      screen.getByText(/You changed where this service lives.*enter the key again/i),
    ).toBeVisible();
    expect(screen.getByRole('button', { name: 'Store API key' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Refresh list' })).toBeDisabled();
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

  it('keeps a persisted credential status request valid across draft edit and revert', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pendingStatus = deferred<ProviderCredentialState>();
    secretStatus.mockReturnValueOnce(pendingStatus.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await waitFor(() => expect(secretStatus).toHaveBeenCalledOnce());

    const modelSelect = await screen.findByRole('combobox', { name: 'Model' });
    await waitFor(() => expect(modelSelect).toHaveValue('llama3.2'));
    await user.selectOptions(modelSelect, CUSTOM_MODEL_OPTION);
    const model = screen.getByRole('textbox', { name: 'Model name' });
    await user.clear(model);
    await user.type(model, 'temporary-model');
    await user.clear(model);
    await user.type(model, 'llama3.2');
    pendingStatus.resolve({
      providerId: 'ollama',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });

    expect(await screen.findByText('Configured')).toBeVisible();
    expect(screen.getByRole('button', { name: 'Replace API key' })).toBeEnabled();
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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));

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
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByText('Configured')).toBeVisible();

    const endpoint = screen.getByRole('textbox', { name: 'Endpoint URL' });
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:32334');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));

    expect(
      await screen.findByText('That did not save. Check the settings and try again.'),
    ).toBeVisible();
    expect(await screen.findByText('Not configured')).toBeVisible();
    expect(secretStatus.mock.calls.length).toBeGreaterThanOrEqual(2);
  });

  it('cancels stale destination checks and ignores their late privacy result', async () => {
    const firstDestination = deferred<'local'>();
    const secondDestination = deferred<'cloud'>();
    // Automatic discovery is quiet: it never checks the destination, so only the credential save
    // and the configuration save contribute a check of their own.
    destination
      .mockReturnValueOnce(firstDestination.promise)
      .mockReturnValueOnce(secondDestination.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    const password = await screen.findByLabelText('API key', { selector: 'input' });
    await user.type(password, 'destination-secret');
    await user.click(screen.getByRole('button', { name: 'Store API key' }));
    await waitFor(() => expect(destination).toHaveBeenCalledOnce());
    const firstOperationId = destination.mock.calls[0]?.[1];

    await user.selectOptions(
      await screen.findByRole('combobox', { name: 'Model' }),
      CUSTOM_MODEL_OPTION,
    );
    await user.type(screen.getByRole('textbox', { name: 'Model name' }), 'manual-model');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(destination).toHaveBeenCalledTimes(2));
    const secondOperationId = destination.mock.calls[1]?.[1];

    expect(cancel).toHaveBeenCalledWith(firstOperationId);
    expect(firstOperationId).not.toBe(secondOperationId);
    secondDestination.resolve('cloud');
    expect(await screen.findByText(/Sends your text to Ollama — checked/)).toBeVisible();
    firstDestination.resolve('local');
    await waitFor(() =>
      expect(screen.getByText(/Sends your text to Ollama — checked/)).toBeVisible(),
    );
    expect(screen.getByText(/cloud provider may charge you for what it processes/i)).toBeVisible();
  });

  it('keeps a newer connection destination authoritative over a late destination-only result', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const staleDestination = deferred<'local'>();
    // Only the configuration save verifies the destination; the automatic discovery on mount is
    // quiet and never claims destination authority.
    destination.mockReturnValueOnce(staleDestination.promise);
    testConnection.mockResolvedValueOnce({ ok: true, destination: 'cloud', modelCount: 1 });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));

    await user.selectOptions(
      await screen.findByRole('combobox', { name: 'Model' }),
      CUSTOM_MODEL_OPTION,
    );
    const model = screen.getByRole('textbox', { name: 'Model name' });
    await user.clear(model);
    await user.type(model, 'new-model');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(destination).toHaveBeenCalledOnce());
    const staleOperationId = destination.mock.calls[0]?.[1];

    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(cancel).toHaveBeenCalledWith(staleOperationId);
    expect(await screen.findByText(/Sends your text to Ollama — checked/)).toBeVisible();
    staleDestination.resolve('local');
    await waitFor(() =>
      expect(screen.getByText(/Sends your text to Ollama — checked/)).toBeVisible(),
    );
    expect(screen.queryByText(/Runs on this computer — checked/)).toBeNull();
  });

  it('settles a connection test when later model discovery claims destination authority', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pendingConnection =
      deferred<Awaited<ReturnType<MainApi['providers']['testConnection']>>>();
    const pendingDestination = deferred<'local'>();
    testConnection.mockReturnValueOnce(pendingConnection.promise);
    destination.mockReturnValueOnce(pendingDestination.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    // The automatic discovery on mount is quiet, so it verifies no destination at all.
    await screen.findByRole('button', { name: 'Refresh list' });
    await waitFor(() => expect(listModels).toHaveBeenCalledOnce());
    expect(destination).not.toHaveBeenCalled();

    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    await waitFor(() => expect(destination).toHaveBeenCalledOnce());
    pendingConnection.resolve({ ok: true, destination: 'cloud', modelCount: 1 });

    expect(
      await screen.findByText(/Connection verified. 1 compatible model reported/),
    ).toBeVisible();
    expect(screen.queryByRole('button', { name: 'Cancel test' })).toBeNull();
    pendingDestination.resolve('local');
    expect(await screen.findByText(/Runs on this computer — checked/)).toBeVisible();
  });

  it('keeps overlapping model and connection cancellation scoped to the right operation', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const pendingModels = deferred<readonly []>();
    const pendingTest = deferred<Awaited<ReturnType<MainApi['providers']['testConnection']>>>();
    testConnection.mockReturnValueOnce(pendingTest.promise);
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    // Let the automatic discovery on mount finish before the explicitly refreshed one is armed.
    await screen.findByRole('button', { name: 'Refresh list' });
    await waitFor(() => expect(listModels).toHaveBeenCalledOnce());
    listModels.mockReturnValueOnce(pendingModels.promise);
    const cancelsBefore = cancel.mock.calls.length;

    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    const modelOperationId = listModels.mock.calls[1]?.[1];
    await user.click(screen.getByRole('button', { name: 'Test connection' }));
    expect(screen.getByText(/Talking to the service… 0\.0s/u)).toBeVisible();
    const connectionOperationId = testConnection.mock.calls[0]?.[1];
    await user.click(screen.getByRole('button', { name: 'Stop searching' }));
    await user.click(screen.getByRole('button', { name: 'Cancel test' }));

    expect(modelOperationId).toBeTruthy();
    expect(connectionOperationId).toBeTruthy();
    expect(modelOperationId).not.toBe(connectionOperationId);
    expect(cancel.mock.calls.slice(cancelsBefore).map(([operationId]) => operationId)).toEqual([
      modelOperationId,
      connectionOperationId,
    ]);
    pendingModels.resolve([]);
    pendingTest.resolve({ ok: true, destination: 'local', modelCount: 0 });
  });

  it('shows loading cancellation and cloud cost qualification', async () => {
    const pending = deferred<readonly []>();
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    // Let the automatic discovery on mount finish before the explicitly refreshed one is armed.
    await screen.findByRole('button', { name: 'Refresh list' });
    await waitFor(() => expect(listModels).toHaveBeenCalledOnce());
    listModels.mockReturnValueOnce(pending.promise);
    const cancelsBefore = cancel.mock.calls.length;
    await user.click(screen.getByRole('button', { name: 'Refresh list' }));
    await user.click(screen.getByRole('button', { name: 'Stop searching' }));
    expect(listModels).toHaveBeenCalledTimes(2);
    const refreshOperationId = listModels.mock.calls[1]?.[1];
    expect(refreshOperationId).toEqual(expect.any(String));
    expect(cancel.mock.calls.slice(cancelsBefore)).toEqual([[refreshOperationId]]);
    pending.resolve([]);
    expect(await screen.findByText('Model discovery cancelled.')).toBeVisible();
    expect(
      screen.queryByText('No models found — you can type a name instead.'),
    ).not.toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: /Ollama.*Run LLMs locally/i }));
    await user.type(screen.getByRole('searchbox', { name: 'Search providers' }), 'OpenAI');
    const openai = screen.getByRole('option', { name: /^OpenAI.*standard option/ });
    await user.click(openai);
    expect(
      await screen.findByText(/cloud provider may charge you for what it processes/i),
    ).toBeVisible();
    expect(screen.getByText(/Sends your text to OpenAI — not checked yet/)).toBeVisible();
    expect(callsFor(destination, 'openai')).toEqual([]);
  });

  it('never contacts a provider whose credentials are not configured', async () => {
    settings = settingsWithConfig(settings, { providerId: 'openai', modelId: 'gpt-4.1-mini' });
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    expect(await screen.findByRole('button', { name: /^OpenAI.*standard option/ })).toBeVisible();
    await waitFor(() => expect(secretStatus).toHaveBeenCalledWith('openai'));
    expect(await screen.findByText('Not configured')).toBeVisible();

    expect(listModels).not.toHaveBeenCalled();
    expect(destination).not.toHaveBeenCalled();

    // The key is what gates it: once stored, discovery arms itself without a discovery click.
    setSecret.mockResolvedValueOnce({
      providerId: 'openai',
      configured: true,
      updatedAt: 1,
      bindingToken: BINDING_TOKEN,
    });
    await user.type(await screen.findByLabelText('API key', { selector: 'input' }), 'sk-test');
    await user.click(screen.getByRole('button', { name: 'Store API key' }));
    await waitFor(() =>
      expect(listModels).toHaveBeenCalledWith('openai', expect.any(String), false),
    );
  });

  it('never contacts a provider for a settings section surfaced only by a search', async () => {
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.type(screen.getByRole('searchbox', { name: 'Search settings' }), 'vision');

    expect(await screen.findByRole('region', { name: 'Smart processing' })).toBeVisible();
    await screen.findByRole('button', { name: /Ollama.*Run LLMs locally/i });
    expect(listModels).not.toHaveBeenCalled();
    expect(destination).not.toHaveBeenCalled();

    // Choosing the section explicitly is what authorises the request.
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await waitFor(() =>
      expect(listModels).toHaveBeenCalledWith('ollama', expect.any(String), false),
    );
  });

  it('re-arms automatic discovery per configuration, not per mount or draft edit', async () => {
    settings.smartProcessing.providers.ollama = {
      ...settings.smartProcessing.providers.ollama,
      modelId: 'llama3.2',
    };
    const user = renderSettings();
    await user.click(screen.getByRole('button', { name: 'Settings' }));
    await user.click(await screen.findByRole('button', { name: 'Smart processing' }));
    await waitFor(() => expect(listModels).toHaveBeenCalledOnce());

    // Navigating away unmounts the section; coming back must not contact the service again.
    await user.click(screen.getByRole('button', { name: 'Recording' }));
    await user.click(screen.getByRole('button', { name: 'Smart processing' }));
    await screen.findByRole('button', { name: 'Refresh list' });
    expect(listModels).toHaveBeenCalledOnce();

    // An edit that is reverted leaves the persisted configuration untouched.
    const endpoint = screen.getByRole('textbox', { name: 'Endpoint URL' });
    await user.type(endpoint, '/');
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:11434');
    await waitFor(() =>
      expect(screen.getByRole('button', { name: 'Save configuration' })).toBeDisabled(),
    );
    expect(listModels).toHaveBeenCalledOnce();

    // A genuine endpoint change, once saved, is a new configuration and arms discovery again.
    await user.clear(endpoint);
    await user.type(endpoint, 'http://127.0.0.1:21434');
    await user.click(screen.getByRole('button', { name: 'Save configuration' }));
    await waitFor(() => expect(listModels).toHaveBeenCalledTimes(2));
  });
});

function callsFor(
  mock: { readonly mock: { readonly calls: readonly (readonly unknown[])[] } },
  providerId: string,
): readonly (readonly unknown[])[] {
  return mock.mock.calls.filter((call) => call[0] === providerId);
}

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
