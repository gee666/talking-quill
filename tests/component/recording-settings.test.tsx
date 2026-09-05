// @vitest-environment jsdom
import { act, cleanup, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RecordingSection } from '../../app/src/renderer/main/settings/RecordingSection';
import type { MainApi } from '../../app/src/shared/bridge/api';
import type { MicrophoneDeviceList, MicrophoneLevel } from '../../app/src/shared/schemas/audio';
import { DEFAULT_SETTINGS, type Settings } from '../../app/src/shared/schemas/settings';

const settings: Settings = structuredClone(DEFAULT_SETTINGS);
const devices: MicrophoneDeviceList = {
  devices: [
    { deviceId: 'default', label: 'System default', isDefault: true },
    { deviceId: 'studio', label: 'Studio microphone', isDefault: false },
  ],
  preferredMicrophoneId: null,
  preferredAvailable: true,
  permission: 'not-determined',
};

const update = vi.fn<MainApi['settings']['update']>();
const getDevices = vi.fn<MainApi['recording']['getDevices']>();
const startTest = vi.fn<MainApi['recording']['startTest']>();
const stopTest = vi.fn<MainApi['recording']['stopTest']>();
const openMicrophoneSettings = vi.fn<MainApi['recording']['openMicrophoneSettings']>();
let levelListener: ((level: MicrophoneLevel) => void) | null = null;

const api = {
  settings: {
    update,
    onChanged: () => () => undefined,
  },
  recording: {
    getDevices,
    startTest,
    stopTest,
    openMicrophoneSettings,
    onDevicesChanged: () => () => undefined,
    onTestLevel: (listener: (level: MicrophoneLevel) => void) => {
      levelListener = listener;
      return () => {
        levelListener = null;
      };
    },
    onTestStateChanged: () => () => undefined,
  },
} as unknown as MainApi;

beforeEach(() => {
  update.mockReset();
  update.mockResolvedValue(settings);
  getDevices.mockReset();
  getDevices.mockResolvedValue(devices);
  startTest.mockReset();
  startTest.mockResolvedValue({
    status: 'active',
    permission: 'granted',
    captureId: 'd9428888-122b-11e1-b85c-61cd3cbb3210',
    activeMicrophoneId: 'default',
    preferredUnavailable: false,
    bindingGeneration: 0,
    sampleRate: 16_000,
    channelCount: 1,
  });
  stopTest.mockReset();
  stopTest.mockResolvedValue({ status: 'idle', permission: 'granted' });
  openMicrophoneSettings.mockReset();
  openMicrophoneSettings.mockResolvedValue();
  levelListener = null;
  Object.defineProperty(window, 'talkingQuill', { configurable: true, value: api });
});

afterEach(cleanup);

function deferred<Value>() {
  let resolvePromise!: (value: Value) => void;
  const promise = new Promise<Value>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

describe('Recording settings', () => {
  it('lists microphones and persists the picker and every silence preset', async () => {
    const user = userEvent.setup();
    render(<RecordingSection settings={settings} platform="win32" />);
    const microphone = await screen.findByRole('combobox', { name: 'Microphone' });
    expect(screen.getByRole('option', { name: 'Studio microphone' })).toBeVisible();
    await user.selectOptions(microphone, 'studio');
    expect(update).toHaveBeenCalledWith({ recording: { preferredMicrophoneId: 'studio' } });
    const preset = screen.getByRole('combobox', { name: 'How long a pause ends a dictation' });
    await user.selectOptions(preset, 'aggressive');
    await user.selectOptions(preset, 'relaxed');
    expect(update).toHaveBeenCalledWith({ recording: { silencePreset: 'aggressive' } });
    expect(update).toHaveBeenCalledWith({ recording: { silencePreset: 'relaxed' } });
    expect(screen.getByRole('option', { name: /^Short pause — /u })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: /^Normal pause — /u })).toBeInTheDocument();
    expect(screen.getByRole('option', { name: /^Long pause — /u })).toBeInTheDocument();
    expect(screen.getByText(/at least 0\.3 seconds/i)).toBeVisible();

    await user.click(screen.getByRole('checkbox', { name: 'Automatically finish after a pause' }));
    await user.click(screen.getByRole('checkbox', { name: 'Include system audio' }));
    expect(update).toHaveBeenCalledWith({ recording: { autoSubmitOnSilence: false } });
    expect(update).toHaveBeenCalledWith({ recording: { includeSystemAudio: true } });
  });

  it.each([
    [
      'How long a pause ends a dictation',
      'relaxed',
      'That pause length couldn’t be saved. Please try again.',
    ],
    ['Automatically finish after a pause', null, 'That finishing option couldn’t be saved.'],
    ['Include system audio', null, 'That audio-source option couldn’t be saved.'],
  ] as const)(
    'keeps the saved value and failure message for %s',
    async (label, option, message) => {
      const user = userEvent.setup();
      update.mockRejectedValueOnce(new Error('private disk detail'));
      render(<RecordingSection settings={settings} platform="win32" />);
      await screen.findByRole('option', { name: 'Studio microphone' });
      const control = screen.getByRole(option === null ? 'checkbox' : 'combobox', { name: label });
      if (option === null) await user.click(control);
      else await user.selectOptions(control, option);

      expect(await screen.findByRole('alert')).toHaveTextContent(message);
      expect(control).toBeEnabled();
      if (option !== null) expect(control).toHaveValue(settings.recording.silencePreset);
      else if (label === 'Automatically finish after a pause') expect(control).toBeChecked();
      else expect(control).not.toBeChecked();
      expect(screen.queryByText('private disk detail')).not.toBeInTheDocument();
      expect(startTest).not.toHaveBeenCalled();
      expect(stopTest).not.toHaveBeenCalled();
    },
  );

  it('shares the recording save lock without starting or stopping microphone capture', async () => {
    const user = userEvent.setup();
    const pending = deferred<Settings>();
    update.mockReturnValueOnce(pending.promise);
    render(<RecordingSection settings={settings} platform="win32" />);
    await screen.findByRole('option', { name: 'Studio microphone' });
    await user.click(screen.getByRole('checkbox', { name: 'Include system audio' }));

    const controls = [
      screen.getByRole('combobox', { name: 'Microphone' }),
      screen.getByRole('combobox', { name: 'How long a pause ends a dictation' }),
      screen.getByRole('checkbox', { name: 'Automatically finish after a pause' }),
      screen.getByRole('checkbox', { name: 'Include system audio' }),
      screen.getByRole('button', { name: 'Test my microphone' }),
    ];
    for (const control of controls) expect(control).toBeDisabled();
    await act(async () => {
      pending.resolve(settings);
      await pending.promise;
    });
    expect(await screen.findByText('System audio will be captured.')).toBeVisible();
    for (const control of controls) expect(control).toBeEnabled();
    expect(startTest).not.toHaveBeenCalled();
    expect(stopTest).not.toHaveBeenCalled();
  });

  it('keeps an active microphone test running while saving recording options', async () => {
    const user = userEvent.setup();
    const pending = deferred<Settings>();
    update.mockReturnValueOnce(pending.promise);
    render(<RecordingSection settings={settings} platform="win32" />);
    await user.click(await screen.findByRole('button', { name: 'Test my microphone' }));
    await screen.findByText('Listening — say something');
    await user.click(screen.getByRole('checkbox', { name: 'Include system audio' }));

    expect(update).toHaveBeenCalledWith({ recording: { includeSystemAudio: true } });
    expect(screen.getByRole('checkbox', { name: 'Include system audio' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Stop test' })).toBeDisabled();
    expect(screen.getByText('Listening — say something')).toBeVisible();
    expect(startTest).toHaveBeenCalledOnce();
    expect(stopTest).not.toHaveBeenCalled();

    await act(async () => {
      pending.resolve(settings);
      await pending.promise;
    });
    expect(await screen.findByText('System audio will be captured.')).toBeVisible();
    expect(screen.getByRole('checkbox', { name: 'Include system audio' })).toBeEnabled();
    expect(screen.getByRole('button', { name: 'Stop test' })).toBeEnabled();
    expect(screen.getByText('Listening — say something')).toBeVisible();
    expect(startTest).toHaveBeenCalledOnce();
    expect(stopTest).not.toHaveBeenCalled();
  });

  it('shows manual finishing and disables unsupported system audio', async () => {
    render(
      <RecordingSection
        settings={{
          ...settings,
          recording: { ...settings.recording, autoSubmitOnSilence: false },
        }}
        platform="darwin"
      />,
    );

    expect(
      await screen.findByRole('checkbox', { name: 'Automatically finish after a pause' }),
    ).not.toBeChecked();
    expect(
      screen.getByRole('combobox', { name: 'How long a pause ends a dictation' }),
    ).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: 'Include system audio' })).toBeDisabled();
    expect(screen.getByText(/available on Windows only/i)).toBeVisible();
  });

  it('lets an unsupported platform turn off a restored system-audio setting', async () => {
    const user = userEvent.setup();
    render(
      <RecordingSection
        settings={{
          ...settings,
          recording: { ...settings.recording, includeSystemAudio: true },
        }}
        platform="darwin"
      />,
    );
    const systemAudio = await screen.findByRole('checkbox', { name: 'Include system audio' });
    expect(systemAudio).toBeChecked();
    expect(systemAudio).toBeEnabled();
    await user.click(systemAudio);
    expect(update).toHaveBeenCalledWith({ recording: { includeSystemAudio: false } });
  });

  it('requests access only on user action, displays live level, and stops cleanly', async () => {
    const user = userEvent.setup();
    const view = render(<RecordingSection settings={settings} platform="win32" />);
    await screen.findByRole('combobox', { name: 'Microphone' });
    expect(startTest).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Test my microphone' }));
    expect(startTest).toHaveBeenCalledOnce();
    expect(await screen.findByText('Listening — say something')).toBeVisible();
    levelListener?.({
      captureId: 'd9428888-122b-11e1-b85c-61cd3cbb3210',
      rms: 0.42,
    });
    await waitFor(() =>
      expect(screen.getByRole('progressbar', { name: 'How loud you are' })).toHaveAttribute(
        'value',
        '0.42',
      ),
    );
    levelListener?.({
      captureId: '11111111-1111-4111-8111-111111111111',
      rms: 0.99,
    });
    expect(screen.getByRole('progressbar', { name: 'How loud you are' })).toHaveAttribute(
      'value',
      '0.42',
    );
    await user.click(screen.getByRole('button', { name: 'Stop test' }));
    expect(stopTest).toHaveBeenCalled();
    view.unmount();
    expect(stopTest).toHaveBeenCalledTimes(2);
  });

  it('visibly reports when a microphone test is using the system-default fallback', async () => {
    const user = userEvent.setup();
    startTest.mockResolvedValueOnce({
      status: 'active',
      permission: 'granted',
      captureId: 'd9428888-122b-11e1-b85c-61cd3cbb3210',
      activeMicrophoneId: 'default',
      preferredUnavailable: true,
      bindingGeneration: 0,
      sampleRate: 16_000,
      channelCount: 1,
    });
    render(
      <RecordingSection
        settings={{
          ...settings,
          recording: { ...settings.recording, preferredMicrophoneId: 'studio' },
        }}
        platform="win32"
      />,
    );

    await user.click(await screen.findByRole('button', { name: 'Test my microphone' }));
    expect(await screen.findByText('Listening on the system default microphone')).toBeVisible();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'Your chosen microphone is unavailable. Talking Quill is using your computer’s current default microphone instead.',
    );
    expect(screen.queryByText(/studio microphone is unavailable/iu)).not.toBeInTheDocument();
  });

  it('allows a pending permission request to be cancelled immediately', async () => {
    const user = userEvent.setup();
    const pending = deferred<Awaited<ReturnType<MainApi['recording']['startTest']>>>();
    startTest.mockReturnValueOnce(pending.promise);
    render(<RecordingSection settings={settings} platform="win32" />);
    await screen.findByRole('combobox', { name: 'Microphone' });

    await user.click(screen.getByRole('button', { name: 'Test my microphone' }));
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    expect(cancel).toBeEnabled();
    await user.click(cancel);
    expect(stopTest).toHaveBeenCalledOnce();

    pending.resolve({ status: 'idle', permission: 'granted' });
    await waitFor(() => expect(stopTest).toHaveBeenCalledTimes(2));
  });

  it('renders already-denied guidance and opens only the main-process settings action', async () => {
    const user = userEvent.setup();
    getDevices.mockResolvedValueOnce({ ...devices, permission: 'denied' });
    render(<RecordingSection settings={settings} platform="darwin" />);
    await screen.findByRole('combobox', { name: 'Microphone' });
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'System Settings → Privacy & Security → Microphone',
    );
    expect(startTest).not.toHaveBeenCalled();
    await user.click(screen.getByRole('button', { name: 'Open microphone settings' }));
    expect(openMicrophoneSettings).toHaveBeenCalledOnce();
  });

  it('does not send Electron authorization failures to Windows privacy settings', async () => {
    const user = userEvent.setup();
    startTest.mockResolvedValueOnce({
      status: 'unavailable',
      permission: 'granted',
      reason: 'permission-unavailable',
    });
    render(<RecordingSection settings={settings} platform="win32" />);
    await user.click(await screen.findByRole('button', { name: 'Test my microphone' }));

    expect(await screen.findByRole('alert')).toHaveTextContent(
      'Talking Quill couldn’t ask for microphone access. Restart Talking Quill and test again.',
    );
    expect(screen.queryByRole('button', { name: 'Open microphone settings' })).toBeNull();
  });

  it('never restarts capture when a device save completes after unmount', async () => {
    const user = userEvent.setup();
    const pending = deferred<Settings>();
    update.mockReturnValueOnce(pending.promise);
    const view = render(<RecordingSection settings={settings} platform="win32" />);
    await screen.findByRole('combobox', { name: 'Microphone' });
    await user.click(screen.getByRole('button', { name: 'Test my microphone' }));
    await screen.findByText('Listening — say something');
    await user.selectOptions(screen.getByRole('combobox', { name: 'Microphone' }), 'studio');
    await vi.waitFor(() => expect(stopTest).toHaveBeenCalledOnce());
    view.unmount();
    pending.resolve(settings);
    await vi.waitFor(() => expect(stopTest).toHaveBeenCalledTimes(2));
    expect(startTest).toHaveBeenCalledOnce();
  });

  it('retains an unavailable preferred device and restores authoritative state after save failure', async () => {
    const user = userEvent.setup();
    update.mockRejectedValueOnce(new Error('private disk detail'));
    render(
      <RecordingSection
        settings={{
          ...settings,
          recording: {
            ...settings.recording,
            preferredMicrophoneId: 'missing',
            silencePreset: 'average',
          },
        }}
        platform="win32"
      />,
    );
    expect(
      await screen.findByRole('option', { name: 'Your chosen microphone (not connected)' }),
    ).toBeVisible();
    await user.selectOptions(screen.getByRole('combobox', { name: 'Microphone' }), 'studio');
    expect(await screen.findByRole('alert')).toHaveTextContent(
      'That microphone couldn’t be saved. Please try again.',
    );
    expect(screen.queryByText('private disk detail')).not.toBeInTheDocument();
  });
});
