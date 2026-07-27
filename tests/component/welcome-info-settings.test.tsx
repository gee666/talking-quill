// @vitest-environment jsdom
/* eslint-disable @typescript-eslint/require-await -- async mocks model the preload Promise API. */
import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { StrictMode } from 'react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { WelcomeWizard } from '../../app/src/renderer/main/welcome/WelcomeWizard';
import { InfoScreen } from '../../app/src/renderer/main/screens/InfoScreen';
import { SettingsScreen } from '../../app/src/renderer/main/screens/SettingsScreen';
import { TranscriptionLanguageSetting } from '../../app/src/renderer/main/settings/TranscriptionModelSection';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

const settings = (): Settings => ({
  ...structuredClone(DEFAULT_SETTINGS),
  welcome: { completedAt: null, lastStep: 1, microphoneTested: false, activationTested: false },
});
const helper = {
  status: 'ready',
  reason: null,
  helperVersion: '1.0.0',
  permissions: { accessibility: 'granted', inputMonitoring: 'granted', eventPost: 'granted' },
} as const;
let api: Record<string, unknown>;

beforeEach(() => {
  api = {
    welcome: {
      setStep: vi.fn(async (step: number) => ({
        completedAt: null,
        lastStep: step,
        reopened: false,
      })),
      complete: vi.fn(async () => ({ completedAt: 10, lastStep: 5, reopened: false })),
    },
    info: {
      status: vi.fn(async () => ({ microphone: 'granted', screenRecording: 'granted', helper })),
      checkForUpdates: vi.fn(async () => ({
        status: 'available',
        currentVersion: '1.0.0',
        latestVersion: '1.1.0',
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
      })),
      cancel: vi.fn(async () => true),
      openPermissionSettings: vi.fn(async () => undefined),
      openLocation: vi.fn(async () => undefined),
      openRelease: vi.fn(async () => undefined),
      notices: vi.fn(async () => 'Mintplex Labs Inc.\nXenova/whisper-small'),
    },
    settings: { update: vi.fn(async () => settings()), onChanged: vi.fn(() => () => undefined) },
    profiles: { update: vi.fn(async () => settings()) },
    data: {
      resetAll: vi.fn(async () => undefined),
      onResetAccepted: vi.fn(() => () => undefined),
    },
    recording: {
      getDevices: vi.fn(async () => ({
        devices: [],
        preferredMicrophoneId: null,
        preferredAvailable: true,
        permission: 'granted',
      })),
      startTest: vi.fn(async () => ({ status: 'idle', permission: 'granted' })),
      stopTest: vi.fn(async () => ({ status: 'idle', permission: 'granted' })),
      openMicrophoneSettings: vi.fn(),
      onDevicesChanged: vi.fn(() => () => undefined),
      onTestLevel: vi.fn(() => () => undefined),
      onTestStateChanged: vi.fn(() => () => undefined),
    },
    models: {
      status: vi.fn(async (modelId: string) => ({
        modelId,
        state: 'ready',
        downloadedBytes: 1,
        totalBytes: 1,
        detail: null,
        repairable: false,
      })),
      download: vi.fn(),
      pause: vi.fn(),
      cancel: vi.fn(),
      retry: vi.fn(),
      delete: vi.fn(),
      onProgress: vi.fn(() => () => undefined),
    },
    activationTest: {
      start: vi.fn(),
      stop: vi.fn(async () => ({
        active: false,
        phase: 'idle',
        profileId: null,
        shortcut: null,
        elapsedMs: 0,
        unavailableReason: null,
      })),
      onChanged: vi.fn(() => () => undefined),
    },
    providers: {
      catalog: vi.fn(async () => []),
      osaStatus: vi.fn(async () => ({
        providerId: 'ollama',
        modelId: null,
        capability: 'unknown',
        manualTestAllowed: false,
        screenPermission: 'granted',
      })),
      cancel: vi.fn(async () => true),
    },
    app: {
      getBootstrap: vi.fn(),
      setEnabled: vi.fn(),
      onStateChanged: vi.fn(() => () => undefined),
    },
    history: {
      list: vi.fn(async () => ({ items: [], nextCursor: null, revision: 0 })),
      onChanged: vi.fn(() => () => undefined),
    },
    commands: { list: vi.fn(async () => []) },
    vocabulary: { list: vi.fn(async () => []) },
    windowControls: { onMaximizedChanged: vi.fn(() => () => undefined) },
    echo: { onSessionChanged: vi.fn(() => () => undefined) },
  };
  Object.defineProperty(window, 'talkingQuill', { configurable: true, value: api });
});
afterEach(cleanup);

const state = { enabled: true, status: 'ready', modelReady: true, helper } as const;

describe('Welcome, Info, and settings completion', () => {
  it('resumes at the persisted step, saves navigation, manages focus, and completes', async () => {
    const user = userEvent.setup();
    const onComplete = vi.fn();
    const resumed = settings();
    resumed.welcome.lastStep = 3;
    render(
      <WelcomeWizard
        settings={resumed}
        state={state}
        platform="win32"
        reopened={false}
        onSettingsSaved={vi.fn()}
        onComplete={onComplete}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByRole('heading', { name: 'Local model' })).toHaveFocus();
    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect((api.welcome as { setStep: ReturnType<typeof vi.fn> }).setStep).toHaveBeenCalledWith(4);
    await user.click(screen.getByRole('button', { name: 'Skip Smart processing' }));
    expect((api.profiles as { update: ReturnType<typeof vi.fn> }).update).toHaveBeenCalledWith(
      'general',
      { processingMode: 'raw' },
    );
    await user.click(screen.getByRole('button', { name: 'Start using Talking Quill' }));
    expect((api.welcome as { complete: ReturnType<typeof vi.fn> }).complete).toHaveBeenCalledOnce();
    expect((api.providers as { catalog: ReturnType<typeof vi.fn> }).catalog).not.toHaveBeenCalled();
    expect(onComplete).toHaveBeenCalledWith({ completedAt: 10, lastStep: 5, reopened: false });
  });

  it('shows and persists auto-detection or a source-language hint during local-model onboarding', async () => {
    const user = userEvent.setup();
    const configured = settings();
    configured.welcome.lastStep = 3;
    configured.transcription.language = 'en';
    const saved = structuredClone(configured);
    saved.transcription.language = 'fr';
    const update = (api.settings as { update: ReturnType<typeof vi.fn> }).update;
    update.mockResolvedValueOnce(saved);
    const onSettingsSaved = vi.fn();

    render(
      <WelcomeWizard
        settings={configured}
        state={state}
        platform="win32"
        reopened={false}
        onSettingsSaved={onSettingsSaved}
        onComplete={vi.fn()}
        onClose={vi.fn()}
      />,
    );

    const language = screen.getByRole('combobox', { name: 'Spoken/source language' });
    expect(language).toHaveValue('en');
    expect(screen.getByText(/without translating it/i)).toBeVisible();
    await user.selectOptions(language, 'fr');
    await user.click(screen.getByRole('button', { name: 'Save source language' }));

    expect(update).toHaveBeenCalledWith({ transcription: { language: 'fr' } });
    expect(onSettingsSaved).toHaveBeenCalledWith(saved);
    expect(await screen.findByText('Source language saved.')).toBeVisible();
  });

  it('uses authoritative step responses and follows persisted progress rollbacks', async () => {
    const user = userEvent.setup();
    const configured = settings();
    configured.welcome.lastStep = 3;
    const setStep = (api.welcome as { setStep: ReturnType<typeof vi.fn> }).setStep;
    setStep.mockResolvedValueOnce({ completedAt: null, lastStep: 2, reopened: false });
    const props = {
      state,
      platform: 'win32',
      reopened: false,
      onSettingsSaved: vi.fn(),
      onComplete: vi.fn(),
      onClose: vi.fn(),
    } as const;
    const view = render(<WelcomeWizard {...props} settings={configured} />);

    await user.click(screen.getByRole('button', { name: 'Continue' }));
    expect(await screen.findByRole('heading', { name: 'Microphone' })).toHaveFocus();

    const advanced = settings();
    advanced.welcome.lastStep = 5;
    view.rerender(<WelcomeWizard {...props} settings={advanced} />);
    expect(await screen.findByRole('heading', { name: 'Ready' })).toHaveFocus();

    const rolledBack = settings();
    rolledBack.welcome.lastStep = 3;
    view.rerender(<WelcomeWizard {...props} settings={rolledBack} />);
    expect(await screen.findByRole('heading', { name: 'Local model' })).toHaveFocus();
  });

  it('does not let an older step response overwrite a newer settings rollback', async () => {
    const user = userEvent.setup();
    let resolveStep!: (value: {
      completedAt: null;
      lastStep: 4;
      revision: 4;
      reopened: false;
    }) => void;
    const setStep = (api.welcome as { setStep: ReturnType<typeof vi.fn> }).setStep;
    setStep.mockReturnValueOnce(
      new Promise((resolve) => {
        resolveStep = resolve;
      }),
    );
    const configured = settings();
    configured.welcome.lastStep = 3;
    configured.welcome.revision = 3;
    const props = {
      state,
      platform: 'win32',
      reopened: false,
      onSettingsSaved: vi.fn(),
      onComplete: vi.fn(),
      onClose: vi.fn(),
    } as const;
    const view = render(<WelcomeWizard {...props} settings={configured} />);

    await user.click(screen.getByRole('button', { name: 'Continue' }));
    const rolledBack = settings();
    rolledBack.welcome.lastStep = 2;
    rolledBack.welcome.revision = 5;
    view.rerender(<WelcomeWizard {...props} settings={rolledBack} />);
    expect(await screen.findByRole('heading', { name: 'Microphone' })).toHaveFocus();

    await act(async () => {
      resolveStep({ completedAt: null, lastStep: 4, revision: 4, reopened: false });
      await Promise.resolve();
    });
    expect(setStep).toHaveBeenCalledOnce();
    expect(screen.getByRole('heading', { name: 'Microphone' })).toHaveFocus();
  });

  it('lists exactly the five Smart built-in reset defaults on the final screen', () => {
    const configured = settings();
    configured.welcome.lastStep = 5;
    const general = configured.dictationProfiles.find((profile) => profile.id === 'general');
    const prompt = configured.dictationProfiles.find((profile) => profile.id === 'prompt');
    if (general === undefined || prompt === undefined) {
      throw new Error('Built-in dictation profiles are missing');
    }
    configured.dictationProfiles = [
      prompt,
      {
        ...general,
        shortcut: {
          modifiers: { ctrl: true, alt: false, shift: true, meta: false },
          keys: ['P'],
        },
        processingMode: 'raw',
      },
      {
        id: '11111111-1111-4111-8111-111111111111',
        name: 'Meeting notes',
        shortcut: {
          modifiers: { ctrl: false, alt: true, shift: false, meta: false },
          keys: ['Y', 'Q'],
        },
        processingMode: 'raw',
        smartPrompt: null,
      },
    ];

    render(
      <WelcomeWizard
        settings={configured}
        state={state}
        platform="win32"
        reopened={false}
        onSettingsSaved={vi.fn()}
        onComplete={vi.fn()}
        onClose={vi.fn()}
      />,
    );
    const profiles = screen.getByRole('list', { name: 'Built-in dictation profile defaults' });
    expect(
      within(profiles)
        .getAllByRole('listitem')
        .map((item) => item.textContent),
    ).toEqual([
      'General: Alt + X (final trigger X) — Smart processing',
      'Prompt: Alt + X + P (final trigger P) — Smart processing',
      'Prompt to English: Alt + X + P + E (final trigger E) — Smart processing',
      'Markdown: Alt + X + M (final trigger M) — Smart processing',
      'Translate to English: Alt + X + E (final trigger E) — Smart processing',
    ]);
    expect(screen.getByText(/existing or migrated profile settings may differ/i)).toBeVisible();
    expect(within(profiles).queryByText('Meeting notes')).not.toBeInTheDocument();
    expect(screen.getByText(/Shortcuts can be changed anytime in Settings/i)).toBeVisible();
    expect(screen.queryByText(/Test activation shortcut/i)).not.toBeInTheDocument();
  });

  it('lets a completed user exit a reopened Welcome flow', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const reopened = settings();
    reopened.welcome.completedAt = 1;
    render(
      <WelcomeWizard
        settings={reopened}
        state={state}
        platform="darwin"
        reopened
        onSettingsSaved={vi.fn()}
        onComplete={vi.fn()}
        onClose={onClose}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Exit Welcome' }));
    expect(onClose).toHaveBeenCalledOnce();
  });

  it('checks updates only on demand, opens notices, and exposes safe local actions', async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    const configured = settings();
    configured.welcome.completedAt = 1;
    render(
      <InfoScreen
        headingRef={{ current: null }}
        bootstrap={{
          appVersion: '1.0.1',
          sourceRevision: 'abcdef123456',
          platform: 'win32',
          state,
          settings: configured,
        }}
        onOpenWelcome={onOpen}
      />,
    );
    expect(screen.getByText('Version 1.0.1 · source abcdef123456')).toBeVisible();
    expect(
      (api.info as { checkForUpdates: ReturnType<typeof vi.fn> }).checkForUpdates,
    ).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(await screen.findByText('Version 1.1.0 is available')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Third-party notices' }));
    expect(await screen.findByText(/Mintplex Labs Inc/)).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Reopen Welcome' }));
    expect(onOpen).toHaveBeenCalledOnce();
  });

  it('clears a previous update result when the next operation fails', async () => {
    const user = userEvent.setup();
    const check = (api.info as { checkForUpdates: ReturnType<typeof vi.fn> }).checkForUpdates;
    check
      .mockResolvedValueOnce({
        status: 'available',
        currentVersion: '1.0.0',
        latestVersion: '1.1.0',
        releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
      })
      .mockRejectedValueOnce(new Error('offline'));
    const configured = settings();
    configured.welcome.completedAt = 1;
    render(
      <InfoScreen
        headingRef={{ current: null }}
        bootstrap={{ appVersion: '1.0.0', platform: 'win32', state, settings: configured }}
        onOpenWelcome={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(await screen.findByText('Version 1.1.0 is available')).toBeVisible();
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(await screen.findByText('offline')).toBeVisible();
    expect(screen.queryByText('Version 1.1.0 is available')).not.toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Open release page' })).not.toBeInTheDocument();
  });

  it('live-announces cancellation and retries with a distinct operation identity', async () => {
    const user = userEvent.setup();
    const check = (api.info as { checkForUpdates: ReturnType<typeof vi.fn> }).checkForUpdates;
    check.mockReturnValueOnce(new Promise(() => undefined)).mockResolvedValueOnce({
      status: 'available',
      currentVersion: '1.0.0',
      latestVersion: '1.1.0',
      releaseUrl: 'https://github.com/gee666/talking-quill/releases/tag/v1.1.0',
    });
    const configured = settings();
    configured.welcome.completedAt = 1;
    render(
      <InfoScreen
        headingRef={{ current: null }}
        bootstrap={{ appVersion: '1.0.0', platform: 'win32', state, settings: configured }}
        onOpenWelcome={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    await user.click(screen.getByRole('button', { name: 'Cancel' }));
    const cancelled = await screen.findByText('Update check cancelled');
    expect(cancelled.closest('[role="status"]')).toHaveAttribute('aria-live', 'polite');
    const cancel = (api.info as { cancel: ReturnType<typeof vi.fn> }).cancel;
    expect(cancel).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: 'Check for updates' }));
    expect(await screen.findByText('Version 1.1.0 is available')).toBeVisible();
    expect(check).toHaveBeenCalledTimes(2);
    expect(check.mock.calls[0]?.[0]).not.toBe(check.mock.calls[1]?.[0]);
    expect(cancel).toHaveBeenCalledWith(check.mock.calls[0]?.[0]);
  });

  it('offers only supported source languages and restores authoritative state on rejection', async () => {
    const user = userEvent.setup();
    const configured = settings();
    configured.transcription.language = 'en';
    const update = vi.fn(() => Promise.reject(new Error('write failed')));
    (api.settings as { update: typeof update }).update = update;
    render(<TranscriptionLanguageSetting settings={configured} onSettingsSaved={vi.fn()} />);
    const input = screen.getByRole('combobox', { name: 'Spoken/source language' });
    await user.selectOptions(input, 'ru');
    fireEvent.click(screen.getByRole('button', { name: 'Save source language' }));
    expect(update).toHaveBeenCalledWith({ transcription: { language: 'ru' } });
    expect(await screen.findByText(/previous value was restored/i)).toBeVisible();
    expect(input).toHaveValue('en');
    expect(screen.getByRole('option', { name: /auto-detect/i })).toBeVisible();
  });

  it('applies language saves after the StrictMode effect replay', async () => {
    const configured = settings();
    configured.transcription.language = 'en';
    const saved = settings();
    saved.transcription.language = 'fr';
    const update = vi.fn(() => Promise.resolve(saved));
    const onSettingsSaved = vi.fn();
    (api.settings as { update: typeof update }).update = update;
    render(
      <StrictMode>
        <TranscriptionLanguageSetting settings={configured} onSettingsSaved={onSettingsSaved} />
      </StrictMode>,
    );

    const input = screen.getByRole('combobox', { name: 'Spoken/source language' });
    fireEvent.change(input, { target: { value: 'fr' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save source language' }));

    await waitFor(() => expect(onSettingsSaved).toHaveBeenCalledWith(saved));
    expect(input).toHaveValue('fr');
  });

  it('keeps the newest authoritative language result when promises settle in reverse order', async () => {
    const configured = settings();
    configured.transcription.language = 'en';
    let rejectFirst!: (error: Error) => void;
    let resolveSecond!: (value: Settings) => void;
    const update = vi
      .fn()
      .mockReturnValueOnce(
        new Promise<Settings>((_resolve, reject) => {
          rejectFirst = reject;
        }),
      )
      .mockReturnValueOnce(
        new Promise<Settings>((resolve) => {
          resolveSecond = resolve;
        }),
      );
    (api.settings as { update: typeof update }).update = update;
    render(<TranscriptionLanguageSetting settings={configured} onSettingsSaved={vi.fn()} />);
    const input = screen.getByRole('combobox', { name: 'Spoken/source language' });
    fireEvent.change(input, { target: { value: 'fr' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save source language' }));
    fireEvent.change(input, { target: { value: 'de' } });
    fireEvent.click(screen.getByRole('button', { name: /source language/i }));
    const newest = settings();
    newest.transcription.language = 'de';
    resolveSecond(newest);
    await waitFor(() => expect(input).toHaveValue('de'));
    rejectFirst(new Error('late failure'));
    await Promise.resolve();
    expect(input).toHaveValue('de');
    expect(screen.queryByText(/could not be saved/i)).not.toBeInTheDocument();
  });

  it('keeps diagnostic logging opt-in and requires the exact reset confirmation', async () => {
    const user = userEvent.setup();
    const configured = settings();
    render(
      <SettingsScreen
        headingRef={{ current: null }}
        settings={configured}
        platform="win32"
        onSettingsSaved={vi.fn()}
        onOpenWelcome={vi.fn()}
      />,
    );
    await user.click(screen.getByRole('button', { name: 'Privacy & data' }));
    const logging = screen.getByRole('checkbox', { name: 'Diagnostic logging' });
    expect(logging).not.toBeChecked();
    await user.click(logging);
    expect((api.settings as { update: ReturnType<typeof vi.fn> }).update).toHaveBeenCalledWith({
      privacy: { diagnosticLoggingEnabled: true },
    });

    await user.click(screen.getByRole('button', { name: 'Reset all application data' }));
    const confirmation = screen.getByRole('textbox', {
      name: 'Type RESET TALKING QUILL to confirm',
    });
    const reset = screen.getByRole('button', { name: 'Reset and restart' });
    expect(reset).toBeDisabled();
    await user.type(confirmation, 'RESET TALKING QUILL');
    expect(reset).toBeEnabled();
    await user.click(reset);
    expect((api.data as { resetAll: ReturnType<typeof vi.fn> }).resetAll).toHaveBeenCalledWith(
      'RESET TALKING QUILL',
    );
    expect(
      await screen.findByText('Reset accepted. Talking Quill will now relaunch.'),
    ).toBeVisible();
  });

  it('filters settings by keywords and reports an empty result accessibly', async () => {
    const user = userEvent.setup();
    const configured = settings();
    configured.welcome.completedAt = 1;
    render(
      <SettingsScreen
        headingRef={{ current: null }}
        settings={configured}
        platform="win32"
        onSettingsSaved={vi.fn()}
        onOpenWelcome={vi.fn()}
      />,
    );
    const search = screen.getByRole('searchbox', { name: 'Search settings' });
    for (const [query, section] of [
      ['language model', 'Transcription model'],
      ['model language', 'Transcription model'],
      ['test connection cloud', 'Smart processing'],
      ['delete retention history', 'Privacy & data'],
      ['snippet phrase trigger', 'Voice Commands'],
      ['export words smart', 'Custom Vocabulary'],
    ] as const) {
      await user.clear(search);
      await user.type(search, query);
      expect(screen.getByRole('region', { name: section })).toBeVisible();
    }
    expect(screen.queryByRole('region', { name: 'General' })).not.toBeInTheDocument();
    await user.clear(search);
    await user.type(search, 'definitely absent');
    await waitFor(() => expect(screen.getByText('No matching settings')).toBeVisible());
  });
});
