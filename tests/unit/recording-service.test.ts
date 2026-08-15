import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  CaptureClientError,
  type CaptureFrame,
  type CaptureStarted,
  type CaptureWindowClient,
  type UnexpectedCaptureStopReason,
} from '../../app/src/main/audio/capture-window-client';
import type { MicrophoneDevice } from '../../app/src/shared/schemas/audio';
import { RecordingService } from '../../app/src/main/audio/recording-service';
import type { IpcEventEmitter } from '../../app/src/main/ipc/event-emitter';
import type { SettingsStore } from '../../app/src/main/persistence/settings-store';
import { DEFAULT_SETTINGS } from '../../app/src/shared/schemas/settings';
import {
  MicrophonePermissionController,
  type MicrophonePermissionPlatform,
} from '../../app/src/main/security/microphone-permission';
import type { SystemAudioCaptureController } from '../../app/src/main/security/system-audio-capture';

class FakeCaptureClient {
  readonly attach = vi.fn();
  readonly listDevices = vi.fn(() =>
    Promise.resolve([{ deviceId: 'default', label: 'Default', isDefault: true }]),
  );
  readonly start = vi.fn<
    (
      preferred: string | null,
      captureId: string,
      includeSystemAudio?: boolean,
    ) => Promise<CaptureStarted>
  >((preferred, captureId, includeSystemAudio) =>
    Promise.resolve({
      captureId,
      activeMicrophoneId: preferred ?? 'default',
      preferredUnavailable: false,
      bindingGeneration: 0,
      systemAudioIncluded: includeSystemAudio === true,
      sampleRate: 16_000,
      channelCount: 1,
    }),
  );
  readonly activate = vi.fn<(captureId: string) => Promise<void>>(() => Promise.resolve());
  readonly rebindDefault = vi.fn((captureId: string, bindingGeneration: number) =>
    Promise.resolve({
      captureId,
      activeMicrophoneId: `rebound-${String(bindingGeneration + 1)}`,
      bindingGeneration: bindingGeneration + 1,
    }),
  );
  readonly stop = vi.fn(() => Promise.resolve());
  readonly reset = vi.fn();
  readonly dispose = vi.fn();
  frameListener: ((frame: CaptureFrame) => void) | null = null;
  deviceListener: ((defaultInvalidated: boolean) => void) | null = null;
  defaultInvalidationListener: ((captureId: string, bindingGeneration: number) => void) | null =
    null;
  stopListener: ((captureId: string, reason: UnexpectedCaptureStopReason) => void) | null = null;

  onFrame(listener: (frame: CaptureFrame) => void): () => void {
    this.frameListener = listener;
    return () => {
      this.frameListener = null;
    };
  }

  onDevicesChanged(listener: (defaultInvalidated: boolean) => void): () => void {
    this.deviceListener = listener;
    return () => {
      this.deviceListener = null;
    };
  }

  onDefaultInvalidated(
    listener: (captureId: string, bindingGeneration: number) => void,
  ): () => void {
    this.defaultInvalidationListener = listener;
    return () => {
      this.defaultInvalidationListener = null;
    };
  }

  onUnexpectedStop(
    listener: (captureId: string, reason: UnexpectedCaptureStopReason) => void,
  ): () => void {
    this.stopListener = listener;
    return () => {
      this.stopListener = null;
    };
  }
}

class FakeOwner extends EventEmitter {
  readonly id = 42;
  isDestroyed(): boolean {
    return false;
  }
}

function deferred<Value>() {
  let resolvePromise!: (value: Value) => void;
  let rejectPromise!: (error: unknown) => void;
  const promise = new Promise<Value>((resolve, reject) => {
    resolvePromise = resolve;
    rejectPromise = reject;
  });
  return { promise, reject: rejectPromise, resolve: resolvePromise };
}

const permissionRequest = {
  webContentsId: 7,
  permission: 'media',
  mediaTypes: ['audio'],
  isMainFrame: true,
  requestingUrl: 'talking-quill://app/capture/index.html',
  requestingOrigin: 'talking-quill://app',
  securityOrigin: 'talking-quill://app',
  embeddingOrigin: null,
  expectedUrl: 'talking-quill://app/capture/index.html',
  expectedOrigin: 'talking-quill://app',
} as const;

function harness(
  options: {
    readonly permission?: 'not-determined' | 'granted' | 'denied' | 'restricted';
    readonly preferredMicrophoneId?: string | null;
    readonly systemAudioSupported?: boolean;
  } = {},
) {
  const capture = new FakeCaptureClient();
  const settings = {
    get: () => ({
      ...structuredClone(DEFAULT_SETTINGS),
      recording: {
        ...structuredClone(DEFAULT_SETTINGS.recording),
        preferredMicrophoneId: options.preferredMicrophoneId ?? null,
        silencePreset: 'average' as const,
      },
    }),
  };
  const events = { send: vi.fn() };
  const permissionPlatform: MicrophonePermissionPlatform = {
    platform: 'win32',
    getStatus: () => options.permission ?? 'granted',
    openExternal: () => Promise.resolve(),
  };
  const permission = new MicrophonePermissionController(permissionPlatform);
  const systemAudio = {
    supported: options.systemAudioSupported ?? true,
    authorize: vi.fn(),
    release: vi.fn(),
    releaseAll: vi.fn(),
  };
  const service = new RecordingService(
    capture as unknown as CaptureWindowClient,
    settings as unknown as SettingsStore,
    events as unknown as IpcEventEmitter,
    permission,
    systemAudio as unknown as SystemAudioCaptureController,
  );
  const captureWebContents = {
    id: 7,
    isDestroyed: () => false,
    reload: vi.fn(),
  };
  service.attachCapture(captureWebContents as unknown as Electron.WebContents);
  return { capture, captureWebContents, events, permission, service, systemAudio };
}

describe('RecordingService ownership', () => {
  it('authorizes and forwards opt-in system audio only for dictation', async () => {
    const test = harness();
    const dictation = await test.service.startDictation(
      { onFrame: vi.fn(), onUnexpectedStop: vi.fn() },
      { includeSystemAudio: true },
    );

    expect(test.systemAudio.authorize).toHaveBeenCalledWith(
      test.captureWebContents,
      dictation.captureId,
    );
    expect(test.capture.start).toHaveBeenCalledWith(null, dictation.captureId, true);
    await test.service.stopDictation(dictation.captureId);
    expect(test.systemAudio.release).toHaveBeenCalledWith(dictation.captureId);
    await test.service.shutdown();
  });

  it('fails instead of silently dropping requested system audio when unsupported', async () => {
    const test = harness({ systemAudioSupported: false });
    await expect(
      test.service.startDictation(
        { onFrame: vi.fn(), onUnexpectedStop: vi.fn() },
        { includeSystemAudio: true },
      ),
    ).rejects.toMatchObject({ code: 'system-audio-unavailable' });
    expect(test.capture.start).not.toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('publishes active state and levels only after activation is acknowledged', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const activation = deferred<undefined>();
    test.capture.activate.mockReturnValueOnce(activation.promise);

    const starting = test.service.startTest(owner as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.activate).toHaveBeenCalledOnce());
    const captureId = test.capture.activate.mock.calls[0]?.[0] ?? '';
    expect(test.capture.listDevices).not.toHaveBeenCalled();
    expect(test.permission.allowsCheck(permissionRequest)).toBe(false);
    expect(test.permission.allowsRequest(permissionRequest)).toBe(false);
    test.capture.frameListener?.({
      captureId,
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.4,
    });
    expect(test.service.getState().status).toBe('starting');
    expect(
      test.events.send.mock.calls.filter(([channel]) => channel === 'recording:test-level'),
    ).toEqual([]);

    activation.resolve(undefined);
    await expect(starting).resolves.toMatchObject({ status: 'active', captureId });
    test.capture.frameListener?.({
      captureId,
      sequence: 1,
      samples: new Float32Array(320),
      rms: 0.4,
    });
    expect(
      test.events.send.mock.calls.filter(([channel]) => channel === 'recording:test-level'),
    ).toEqual([['recording:test-level', expect.objectContaining({ rms: 0.4 })]]);
    await test.service.shutdown();
  });
  it('makes a microphone test active while authorized device enumeration is still pending', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const owner = new FakeOwner();
    const enumeration = deferred<MicrophoneDevice[]>();
    test.capture.listDevices.mockReturnValueOnce(enumeration.promise);

    await expect(
      test.service.startTest(owner as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'active' });
    expect(test.capture.activate).toHaveBeenCalledOnce();
    expect(test.permission.allowsCheck(permissionRequest)).toBe(true);
    expect(test.permission.allowsRequest(permissionRequest)).toBe(false);

    enumeration.resolve([{ deviceId: 'studio', label: 'Studio', isDefault: false }]);
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(false));
    expect(test.events.send).toHaveBeenCalledWith(
      'recording:devices-changed',
      expect.objectContaining({ devices: [expect.objectContaining({ deviceId: 'studio' })] }),
    );
    await test.service.shutdown();
  });

  it('enumerates after acquisition and seals authorization after enumeration', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const owner = new FakeOwner();
    await expect(
      test.service.startTest(owner as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'active' });
    expect(test.capture.listDevices).toHaveBeenCalledOnce();
    expect(test.permission.allowsCheck(permissionRequest)).toBe(false);
    expect(test.permission.allowsRequest(permissionRequest)).toBe(false);
    await test.service.shutdown();
  });

  it('rebinds an active default test in place and resets its evidence metadata', async () => {
    const test = harness();
    const invalidateMicrophone = vi.fn();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.frameListener?.({
      captureId: state.captureId,
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.4,
    });
    expect(test.service.microphoneTestObservation()).toMatchObject({
      observedRms: 0.4,
      sampleCount: 320,
    });

    test.capture.defaultInvalidationListener?.(state.captureId, 0);
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledOnce());
    await vi.waitFor(() =>
      expect(test.service.getState()).toMatchObject({
        status: 'active',
        captureId: state.captureId,
        activeMicrophoneId: 'rebound-1',
        bindingGeneration: 1,
      }),
    );
    expect(test.capture.start).toHaveBeenCalledOnce();
    expect(test.capture.activate).toHaveBeenCalledOnce();
    expect(test.service.microphoneTestObservation()).toMatchObject({
      boundDeviceId: 'rebound-1',
      observedRms: 0,
      sampleCount: 0,
    });
    expect(test.events.send).toHaveBeenCalledWith('recording:test-level', {
      captureId: state.captureId,
      rms: 0,
    });
    expect(invalidateMicrophone).toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('coalesces binding races into one follow-up rebind', async () => {
    const test = harness();
    const firstRebind = deferred<{
      captureId: string;
      activeMicrophoneId: string;
      bindingGeneration: number;
    }>();
    test.capture.rebindDefault.mockReturnValueOnce(firstRebind.promise);
    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');

    test.service.invalidateInputDevices();
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledOnce());
    test.capture.defaultInvalidationListener?.(state.captureId, 0);
    test.capture.defaultInvalidationListener?.(state.captureId, 1);
    expect(test.capture.rebindDefault).toHaveBeenCalledOnce();

    firstRebind.resolve({
      captureId: state.captureId,
      activeMicrophoneId: 'replacement-one',
      bindingGeneration: 1,
    });
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledTimes(2));
    expect(test.capture.rebindDefault).toHaveBeenLastCalledWith(state.captureId, 1);
    await vi.waitFor(() => expect(test.service.getState()).toMatchObject({ bindingGeneration: 2 }));
    await test.service.shutdown();
  });

  it('invalidates idle explicit evidence when current presence cannot be authorized', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const invalidateMicrophone = vi.fn();
    const validationChanged = vi.fn<(known: boolean) => void>();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    test.service.setWelcomeEvidenceValidationListener(validationChanged);
    test.capture.listDevices.mockResolvedValue([
      { deviceId: 'studio', label: 'Studio', isDefault: false },
    ]);

    test.capture.deviceListener?.(false);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    await vi.waitFor(() => expect(invalidateMicrophone).toHaveBeenCalledOnce());
    expect(validationChanged).toHaveBeenCalledWith(false);
    expect(test.service.microphoneReadyForWelcome()).toBe(false);
    await test.service.shutdown();
  });

  it('drains a native default invalidation received while activation is pending', async () => {
    const test = harness();
    const activation = deferred<undefined>();
    test.capture.activate.mockReturnValueOnce(activation.promise);
    const starting = test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.activate).toHaveBeenCalledOnce());
    const captureId = test.capture.activate.mock.calls[0]?.[0] ?? '';

    test.service.invalidateInputDevices();
    expect(test.capture.rebindDefault).not.toHaveBeenCalled();
    activation.resolve(undefined);
    await expect(starting).resolves.toMatchObject({ status: 'active', captureId });
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledOnce());
    await test.service.shutdown();
  });

  it('does not re-invalidate rebound evidence from the delayed device-list signal', async () => {
    const test = harness();
    const invalidateMicrophone = vi.fn();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.defaultInvalidationListener?.(state.captureId, 0);
    await vi.waitFor(() => expect(test.service.getState()).toMatchObject({ bindingGeneration: 1 }));
    invalidateMicrophone.mockClear();
    test.capture.frameListener?.({
      captureId: state.captureId,
      sequence: 1,
      samples: new Float32Array(320),
      rms: 0.4,
    });

    test.capture.deviceListener?.(true);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(3));
    expect(invalidateMicrophone).not.toHaveBeenCalled();
    expect(test.service.microphoneTestObservation()).toMatchObject({
      observedRms: 0.4,
      sampleCount: 320,
    });
    await test.service.shutdown();
  });

  it('never rebinds or invalidates an active explicit microphone that remains available', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const studio = { deviceId: 'studio', label: 'Studio', isDefault: false };
    const invalidateMicrophone = vi.fn();
    const validationChanged = vi.fn<(known: boolean) => void>();
    test.capture.listDevices.mockResolvedValue([studio]);
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    test.service.setWelcomeEvidenceValidationListener(validationChanged);
    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    test.capture.frameListener?.({
      captureId: state.captureId,
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.4,
    });

    test.service.invalidateInputDevices();
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(2));
    expect(test.capture.rebindDefault).not.toHaveBeenCalled();
    expect(invalidateMicrophone).not.toHaveBeenCalled();
    expect(validationChanged.mock.calls.map(([known]) => known)).toEqual([false, true]);
    expect(test.service.microphoneTestObservation()).toMatchObject({
      boundDeviceId: 'studio',
      observedRms: 0.4,
      sampleCount: 320,
    });
    await test.service.shutdown();
  });

  it('fails a default rebind after the bounded retry budget', async () => {
    const test = harness();
    const onUnexpectedStop = vi.fn();
    const dictation = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop,
    });
    test.capture.rebindDefault.mockRejectedValue(new CaptureClientError('device-unavailable'));
    test.capture.stop.mockRejectedValueOnce(new Error('capture renderer stopped responding'));

    test.capture.defaultInvalidationListener?.(dictation.captureId, 0);
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledTimes(2));
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(dictation.captureId));
    await vi.waitFor(() => expect(onUnexpectedStop).toHaveBeenCalledWith('device-unavailable'));
    expect(test.capture.reset).toHaveBeenCalledOnce();
    await test.service.shutdown();
  });

  it('keeps dictation ownership and PCM routing across a successful default rebind', async () => {
    const test = harness();
    const onFrame = vi.fn();
    const onUnexpectedStop = vi.fn();
    const dictation = await test.service.startDictation({ onFrame, onUnexpectedStop });

    test.capture.defaultInvalidationListener?.(dictation.captureId, 0);
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledOnce());
    const samples = new Float32Array(320).fill(0.25);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 10,
      samples,
      rms: 0.25,
    });
    expect(onFrame).toHaveBeenCalledWith(samples, 0.25);
    expect(onUnexpectedStop).not.toHaveBeenCalled();
    await test.service.stopDictation(dictation.captureId);
    expect(test.capture.stop).toHaveBeenCalledWith(dictation.captureId);
    await test.service.shutdown();
  });

  it('stops the capture and clears the lease when the owning WebContents is destroyed', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    expect(state.status).toBe('active');
    expect(test.capture.activate).toHaveBeenCalledOnce();
    const captureId = state.status === 'active' ? state.captureId : '';
    owner.emit('destroyed');
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(captureId));
    await vi.waitFor(() => expect(test.service.getState().status).toBe('idle'));
    await test.service.shutdown();
  });

  it('forces a capture reset and reload instead of releasing ownership after a failed stop', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.stop.mockRejectedValueOnce(new Error('capture renderer unresponsive'));

    await expect(test.service.stopTest(owner.id)).resolves.toMatchObject({
      status: 'unavailable',
      reason: 'capture-unavailable',
    });
    expect(test.capture.reset).toHaveBeenCalledOnce();
    expect(test.captureWebContents.reload).toHaveBeenCalledOnce();
    expect(test.permission.allowsCheck(permissionRequest)).toBe(false);
    await test.service.shutdown();
  });

  it('clears stop coalescing even when forced transport reset cleanup throws', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.stop.mockRejectedValueOnce(new Error('capture renderer unresponsive'));
    test.capture.reset.mockImplementationOnce(() => {
      throw new Error('port already gone');
    });
    test.captureWebContents.reload.mockImplementationOnce(() => {
      throw new Error('renderer already gone');
    });

    await expect(test.service.stopTest(owner.id)).resolves.toMatchObject({ status: 'unavailable' });
    const replacement = { id: 8, isDestroyed: () => false, reload: vi.fn() };
    test.service.attachCapture(replacement as unknown as Electron.WebContents);
    await expect(
      test.service.startTest(owner as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'active' });
    expect(test.capture.start).toHaveBeenCalledTimes(2);
    await test.service.shutdown();
  });

  it('cancels a pending capture start out of band instead of queueing stop behind it', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const pending = deferred<CaptureStarted>();
    test.capture.start.mockReturnValueOnce(pending.promise);
    const starting = test.service.startTest(owner as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.start).toHaveBeenCalledOnce());
    const captureId = test.capture.start.mock.calls[0]?.[1] ?? '';

    await expect(test.service.stopTest(owner.id)).resolves.toMatchObject({ status: 'idle' });
    expect(test.capture.stop).toHaveBeenCalledWith(captureId);

    pending.resolve({
      captureId,
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
      bindingGeneration: 0,
      systemAudioIncluded: false,
      sampleRate: 16_000,
      channelCount: 1,
    });
    await expect(starting).resolves.toMatchObject({ status: 'idle' });
    await test.service.shutdown();
  });

  it('uses a bounded reset fallback when capture cancellation never settles', async () => {
    vi.useFakeTimers();
    try {
      const test = harness();
      const owner = new FakeOwner();
      const state = await test.service.startTest(owner as unknown as Electron.WebContents);
      if (state.status !== 'active') throw new Error('Expected an active test');
      test.capture.stop.mockReturnValueOnce(new Promise(() => undefined));

      const stopping = test.service.stopTest(owner.id);
      await vi.advanceTimersByTimeAsync(1_000);
      await expect(stopping).resolves.toMatchObject({
        status: 'unavailable',
        reason: 'capture-unavailable',
      });
      expect(test.capture.reset).toHaveBeenCalledOnce();
      expect(test.captureWebContents.reload).toHaveBeenCalledOnce();
      await test.service.shutdown();
    } finally {
      vi.useRealTimers();
    }
  });

  it('stops and releases an active test when its main renderer starts reloading', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');

    owner.emit('did-start-navigation', {}, 'talking-quill://app/main/index.html', false, true);

    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(state.captureId));
    await vi.waitFor(() => expect(test.service.getState().status).toBe('idle'));
    expect(owner.listenerCount('did-start-navigation')).toBe(0);
    await test.service.shutdown();
  });

  it('does not enumerate on hidden capture attach and initializes denied state without capture', async () => {
    const test = harness({ permission: 'denied' });
    expect(test.capture.listDevices).not.toHaveBeenCalled();
    const refreshEventsBefore = test.events.send.mock.calls.filter(
      ([channel]) => channel === 'recording:devices-changed',
    ).length;
    await test.service.getDevices();
    const refreshEventsAfter = test.events.send.mock.calls.filter(
      ([channel]) => channel === 'recording:devices-changed',
    ).length;
    expect(refreshEventsAfter).toBe(refreshEventsBefore + 1);

    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({
      status: 'blocked',
      permission: 'denied',
    });
    expect(test.capture.start).not.toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('distinguishes Electron policy rejection from an OS privacy denial', async () => {
    const test = harness({ permission: 'granted' });
    const owner = new FakeOwner();
    const pending = deferred<CaptureStarted>();
    test.capture.start.mockReturnValueOnce(pending.promise);
    const starting = test.service.startTest(owner as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.start).toHaveBeenCalledOnce());
    const captureId = test.capture.start.mock.calls[0]?.[1] ?? '';
    const malformed = { ...permissionRequest, securityOrigin: 'https://attacker.invalid' };
    expect(test.permission.allowsRequest(malformed)).toBe(false);
    test.permission.notePolicyDenied(malformed);
    pending.reject(new CaptureClientError('permission-denied'));

    await expect(starting).resolves.toMatchObject({
      status: 'unavailable',
      permission: 'granted',
      reason: 'permission-unavailable',
    });
    expect(test.permission.takePolicyDenial(captureId)).toBe(false);
    await test.service.shutdown();
  });

  it.each(['no-device', 'device-unavailable'] as const)(
    'invalidates prior evidence when microphone startup fails with %s',
    async (code) => {
      const test = harness();
      const invalidateMicrophone = vi.fn();
      const validationChanged = vi.fn<(known: boolean) => void>();
      test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
      test.service.setWelcomeEvidenceValidationListener(validationChanged);
      test.capture.start.mockRejectedValueOnce(new CaptureClientError(code));

      await expect(
        test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
      ).resolves.toMatchObject({ status: 'unavailable', reason: code });
      expect(invalidateMicrophone).toHaveBeenCalledOnce();
      expect(validationChanged).toHaveBeenCalledWith(false);
      expect(test.service.microphoneReadyForWelcome()).toBe(false);
      await test.service.shutdown();
    },
  );

  it('keeps fallback capture active without treating it as explicit-device evidence', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const invalidateMicrophone = vi.fn();
    const validationChanged = vi.fn<(known: boolean) => void>();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    test.service.setWelcomeEvidenceValidationListener(validationChanged);
    test.capture.start.mockImplementationOnce((_preferred, captureId) => {
      expect(test.permission.allowsRequest(permissionRequest)).toBe(true);
      expect(test.permission.allowsRequest(permissionRequest)).toBe(true);
      expect(test.permission.allowsRequest(permissionRequest)).toBe(false);
      return Promise.resolve({
        captureId,
        activeMicrophoneId: 'current-default',
        preferredUnavailable: true,
        bindingGeneration: 0,
        systemAudioIncluded: false,
        sampleRate: 16_000,
        channelCount: 1,
      });
    });

    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    expect(state).toMatchObject({
      activeMicrophoneId: 'current-default',
      preferredUnavailable: true,
    });
    test.capture.frameListener?.({
      captureId: state.captureId,
      sequence: 0,
      samples: new Float32Array(1_600),
      rms: 0.3,
    });

    expect(test.service.microphoneTestObservation()).toBeNull();
    expect(test.service.microphoneReadyForWelcome()).toBe(false);
    expect(validationChanged).toHaveBeenCalledWith(false);
    expect(invalidateMicrophone).toHaveBeenCalled();
    await expect(test.service.getDevices()).resolves.toMatchObject({
      preferredMicrophoneId: 'studio',
      preferredAvailable: false,
    });

    test.capture.defaultInvalidationListener?.(state.captureId, 0);
    await vi.waitFor(() => expect(test.capture.rebindDefault).toHaveBeenCalledOnce());
    expect(test.service.getState()).toMatchObject({
      status: 'active',
      preferredUnavailable: true,
      bindingGeneration: 1,
    });
    await test.service.shutdown();
  });

  it('accepts matching exact-capture evidence when ancillary enumeration fails', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const invalidateMicrophone = vi.fn();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    test.capture.listDevices.mockRejectedValueOnce(new Error('enumeration failed'));

    const state = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    test.capture.frameListener?.({
      captureId: state.captureId,
      sequence: 0,
      samples: new Float32Array(1_600),
      rms: 0.2,
    });

    expect(test.service.microphoneTestObservation()).toEqual({
      boundDeviceId: 'studio',
      observedRms: 0.2,
      sampleCount: 1_600,
    });
    expect(test.service.microphoneReadyForWelcome()).toBe(true);
    expect(invalidateMicrophone).not.toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('does not claim Windows denial for an unexplained NotAllowedError', async () => {
    const test = harness({ permission: 'granted' });
    test.capture.start.mockRejectedValueOnce(new CaptureClientError('permission-denied'));
    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({
      status: 'unavailable',
      permission: 'granted',
      reason: 'permission-unavailable',
    });
    await test.service.shutdown();
  });

  it('does not erase authorized Bluetooth metadata on a permission-hidden hot-plug refresh', async () => {
    const test = harness();
    const bluetooth = {
      deviceId: 'bluetooth-headset',
      label: 'Bluetooth Hands-Free Microphone',
      isDefault: false,
    };
    test.capture.listDevices.mockResolvedValueOnce([bluetooth]);
    const active = await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    if (active.status !== 'active') throw new Error('Expected an active test');
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    await test.service.stopTest();
    test.capture.listDevices.mockResolvedValue([]);
    test.capture.deviceListener?.(false);

    await expect(test.service.getDevices()).resolves.toMatchObject({ devices: [bluetooth] });
    await test.service.shutdown();
  });

  it('serializes dirty device refreshes and publishes the newest authorized generation', async () => {
    const test = harness();
    const first = deferred<MicrophoneDevice[]>();
    const second = deferred<MicrophoneDevice[]>();
    test.capture.listDevices.mockReset();
    test.capture.listDevices.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);

    await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    test.capture.deviceListener?.(false);
    const dirty = test.service.getDevices();
    expect(test.capture.listDevices).toHaveBeenCalledOnce();

    first.resolve([{ deviceId: 'first', label: 'First', isDefault: false }]);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(2));
    let dirtySettled = false;
    void dirty.then(() => {
      dirtySettled = true;
    });
    await Promise.resolve();
    expect(dirtySettled).toBe(false);
    second.resolve([{ deviceId: 'second', label: 'Second', isDefault: false }]);
    await expect(dirty).resolves.toMatchObject({
      devices: [expect.objectContaining({ deviceId: 'second' })],
    });
    await test.service.shutdown();
  });

  it('does not publish or invalidate from a superseded device enumeration', async () => {
    const test = harness({ preferredMicrophoneId: 'studio' });
    const studio = { deviceId: 'studio', label: 'Studio', isDefault: false };
    test.capture.listDevices.mockResolvedValueOnce([studio]);
    await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    const invalidateMicrophone = vi.fn();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    const stale = deferred<MicrophoneDevice[]>();
    const current = deferred<MicrophoneDevice[]>();
    test.capture.listDevices
      .mockReturnValueOnce(stale.promise)
      .mockReturnValueOnce(current.promise);
    const eventsBefore = test.events.send.mock.calls.length;

    const first = test.service.getDevices();
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(2));
    test.capture.deviceListener?.(false);
    let firstSettled = false;
    void first.then(() => {
      firstSettled = true;
    });
    stale.resolve([]);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(3));
    expect(firstSettled).toBe(false);
    expect(test.events.send.mock.calls).toHaveLength(eventsBefore + 1);
    expect(test.events.send).toHaveBeenLastCalledWith(
      'recording:devices-changed',
      expect.objectContaining({ preferredAvailable: false }),
    );
    expect(invalidateMicrophone).not.toHaveBeenCalled();

    current.resolve([studio]);
    await first;
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(3));
    expect(test.events.send).toHaveBeenLastCalledWith(
      'recording:devices-changed',
      expect.objectContaining({ preferredAvailable: true }),
    );
    expect(invalidateMicrophone).not.toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('clears stale devices only from an authorized empty snapshot', async () => {
    const test = harness();
    const stale = { deviceId: 'stale', label: 'Stale', isDefault: false };
    test.capture.listDevices.mockResolvedValueOnce([stale]);
    await test.service.startTest(new FakeOwner() as unknown as Electron.WebContents);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledOnce());
    await expect(test.service.getDevices()).resolves.toMatchObject({ devices: [stale] });

    test.capture.listDevices.mockResolvedValueOnce([]);
    test.capture.deviceListener?.(false);
    await vi.waitFor(() => expect(test.capture.listDevices).toHaveBeenCalledTimes(3));
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(false));
    test.capture.listDevices.mockResolvedValue([]);
    await expect(test.service.getDevices()).resolves.toMatchObject({ devices: [] });
    await test.service.shutdown();
  });

  it('suppresses unchanged device snapshots after the initial publication', async () => {
    const test = harness();
    await test.service.getDevices();
    const initialEvents = test.events.send.mock.calls.filter(
      ([channel]) => channel === 'recording:devices-changed',
    ).length;
    await test.service.getDevices();
    expect(
      test.events.send.mock.calls.filter(([channel]) => channel === 'recording:devices-changed'),
    ).toHaveLength(initialEvents);
    await test.service.shutdown();
  });

  it('continues dictation on the default microphone when the explicit preference falls back', async () => {
    const test = harness({ preferredMicrophoneId: 'stale-device' });
    const onFrame = vi.fn();
    test.capture.start.mockImplementationOnce((_preferred, captureId) =>
      Promise.resolve({
        captureId,
        activeMicrophoneId: 'current-default',
        preferredUnavailable: true,
        bindingGeneration: 0,
        systemAudioIncluded: false,
        sampleRate: 16_000,
        channelCount: 1,
      }),
    );

    const dictation = await test.service.startDictation({
      onFrame,
      onUnexpectedStop: vi.fn(),
    });
    expect(dictation.activeMicrophoneId).toBe('current-default');
    const samples = new Float32Array(320).fill(0.2);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 0,
      samples,
      rms: 0.2,
    });
    expect(onFrame).toHaveBeenCalledWith(samples, 0.2);
    expect(test.service.microphoneReadyForWelcome()).toBe(false);
    await test.service.shutdown();
  });

  it('makes dictation usable while authorized device enumeration is still pending', async () => {
    const test = harness();
    const enumeration = deferred<MicrophoneDevice[]>();
    test.capture.listDevices.mockReturnValueOnce(enumeration.promise);
    const onFrame = vi.fn();

    const dictation = await test.service.startDictation({
      onFrame,
      onUnexpectedStop: vi.fn(),
    });
    expect(test.capture.activate).toHaveBeenCalledWith(dictation.captureId);
    expect(test.permission.allowsCheck(permissionRequest)).toBe(true);
    const samples = new Float32Array(320).fill(0.25);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 0,
      samples,
      rms: 0.25,
    });
    expect(onFrame).toHaveBeenCalledWith(samples, 0.25);

    enumeration.resolve([{ deviceId: 'default', label: 'Default', isDefault: true }]);
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(false));
    await test.service.shutdown();
  });

  it('ignores stale ancillary device results without sealing the current lease', async () => {
    const test = harness();
    const firstEnumeration = deferred<MicrophoneDevice[]>();
    const secondEnumeration = deferred<MicrophoneDevice[]>();
    test.capture.listDevices
      .mockReturnValueOnce(firstEnumeration.promise)
      .mockReturnValueOnce(secondEnumeration.promise);
    const firstOnFrame = vi.fn();
    const first = await test.service.startDictation({
      onFrame: firstOnFrame,
      onUnexpectedStop: vi.fn(),
    });
    await test.service.stopDictation(first.captureId);
    const secondOnFrame = vi.fn();
    const second = await test.service.startDictation({
      onFrame: secondOnFrame,
      onUnexpectedStop: vi.fn(),
    });

    test.capture.frameListener?.({
      captureId: first.captureId,
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.1,
    });
    expect(firstOnFrame).not.toHaveBeenCalled();
    expect(secondOnFrame).not.toHaveBeenCalled();
    test.capture.deviceListener?.(false);
    expect(test.events.send).not.toHaveBeenCalledWith(
      'recording:devices-changed',
      expect.objectContaining({
        devices: [expect.objectContaining({ deviceId: 'stale-event' })],
      }),
    );

    firstEnumeration.resolve([
      { deviceId: 'stale-device', label: 'Stale device', isDefault: false },
    ]);
    await firstEnumeration.promise;
    await Promise.resolve();
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(true));
    expect(test.events.send).not.toHaveBeenCalledWith(
      'recording:devices-changed',
      expect.objectContaining({
        devices: [expect.objectContaining({ deviceId: 'stale-device' })],
      }),
    );

    secondEnumeration.resolve([
      { deviceId: 'current-device', label: 'Current device', isDefault: false },
    ]);
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(false));
    expect(test.events.send).toHaveBeenCalledWith(
      'recording:devices-changed',
      expect.objectContaining({
        devices: [expect.objectContaining({ deviceId: 'current-device' })],
      }),
    );
    await test.service.stopDictation(second.captureId);
    await test.service.shutdown();
  });

  it('seals ancillary enumeration authorization when enumeration fails', async () => {
    const test = harness();
    test.capture.listDevices.mockRejectedValueOnce(new Error('enumeration failed'));

    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'active' });
    await vi.waitFor(() => expect(test.permission.allowsCheck(permissionRequest)).toBe(false));
    await test.service.shutdown();
  });

  it('routes final dictation PCM until stop acknowledgement and coalesces stop callers', async () => {
    const test = harness();
    const onFrame = vi.fn();
    const dictation = await test.service.startDictation({
      onFrame,
      onUnexpectedStop: vi.fn(),
    });
    const stopped = deferred<undefined>();
    test.capture.stop.mockReturnValueOnce(stopped.promise);

    const firstStop = test.service.stopDictation(dictation.captureId);
    const secondStop = test.service.stopDictation(dictation.captureId);
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledOnce());
    const finalSamples = Float32Array.from([0.25, -0.25]);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 0,
      samples: finalSamples,
      rms: 0.25,
    });
    expect(onFrame).toHaveBeenCalledWith(finalSamples, 0.25);

    stopped.resolve(undefined);
    await Promise.all([firstStop, secondStop]);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 1,
      samples: new Float32Array([0.5]),
      rms: 0.5,
    });
    expect(onFrame).toHaveBeenCalledOnce();
    await test.service.shutdown();
  });

  it('routes dictation PCM exclusively and prevents microphone tests from preempting it', async () => {
    const test = harness();
    const onFrame = vi.fn();
    const onUnexpectedStop = vi.fn();
    const dictation = await test.service.startDictation({ onFrame, onUnexpectedStop });
    const samples = new Float32Array(320).fill(0.25);
    test.capture.frameListener?.({
      captureId: dictation.captureId,
      sequence: 0,
      samples,
      rms: 0.25,
    });
    expect(onFrame).toHaveBeenCalledWith(samples, 0.25);
    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'unavailable', reason: 'capture-unavailable' });
    await test.service.stopTest();
    expect(test.capture.stop).not.toHaveBeenCalled();
    await test.service.stopDictation(dictation.captureId);
    expect(test.capture.stop).toHaveBeenCalledWith(dictation.captureId);
    await test.service.shutdown();
  });

  it('does not let stale dictation cleanup stop an active microphone test', async () => {
    const test = harness();
    const dictation = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await test.service.stopDictation(dictation.captureId);
    const owner = new FakeOwner();
    await expect(
      test.service.startTest(owner as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'active' });
    const stopCalls = test.capture.stop.mock.calls.length;

    await test.service.stopDictation();
    await test.service.stopDictation(dictation.captureId);

    expect(test.capture.stop).toHaveBeenCalledTimes(stopCalls);
    expect(test.service.getState()).toMatchObject({ status: 'active' });
    expect(test.service.microphoneTestObservation()).not.toBeNull();
    await test.service.shutdown();
  });

  it('coalesces stale draining-dictation cleanup without cancelling the next test', async () => {
    const test = harness();
    const dictation = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    const stopped = deferred<undefined>();
    test.capture.stop.mockReturnValueOnce(stopped.promise);

    const firstStop = test.service.stopDictation(dictation.captureId);
    const owner = new FakeOwner();
    const startingTest = test.service.startTest(owner as unknown as Electron.WebContents);
    const staleStop = test.service.stopDictation(dictation.captureId);
    stopped.resolve(undefined);

    await Promise.all([firstStop, staleStop]);
    await expect(startingTest).resolves.toMatchObject({ status: 'active' });
    expect(test.capture.start).toHaveBeenCalledTimes(2);
    expect(test.service.getState()).toMatchObject({ status: 'active' });
    await test.service.shutdown();
  });

  it('protects a queued dictation while the previous microphone test is stopping', async () => {
    const test = harness();
    const firstOwner = new FakeOwner();
    const activeTest = await test.service.startTest(firstOwner as unknown as Electron.WebContents);
    if (activeTest.status !== 'active') throw new Error('Expected an active test');
    const stopped = deferred<undefined>();
    test.capture.stop.mockReturnValueOnce(stopped.promise);

    const startingDictation = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(activeTest.captureId));
    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'unavailable', reason: 'capture-unavailable' });
    await test.service.stopTest();

    stopped.resolve(undefined);
    const dictation = await startingDictation;
    expect(test.capture.start).toHaveBeenCalledTimes(2);
    await test.service.stopDictation(dictation.captureId);
    await test.service.shutdown();
  });

  it('cancels a queued dictation without disturbing the microphone-test stop', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const activeTest = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (activeTest.status !== 'active') throw new Error('Expected an active test');
    const stopped = deferred<undefined>();
    test.capture.stop.mockReturnValueOnce(stopped.promise);

    const startingDictation = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(activeTest.captureId));
    const cancelDictation = test.service.stopDictation();
    stopped.resolve(undefined);
    await cancelDictation;

    await expect(startingDictation).rejects.toMatchObject({ code: 'capture-unavailable' });
    expect(test.capture.start).toHaveBeenCalledOnce();
    expect(test.service.getState()).toMatchObject({ status: 'idle' });
    await test.service.shutdown();
  });

  it('cancels a replacement dictation queued behind the previous dictation drain', async () => {
    const test = harness();
    const first = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    const stopped = deferred<undefined>();
    test.capture.stop.mockReturnValueOnce(stopped.promise);

    const replacement = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await vi.waitFor(() => expect(test.capture.stop).toHaveBeenCalledWith(first.captureId));
    const cancelReplacement = test.service.stopDictation();
    stopped.resolve(undefined);
    await cancelReplacement;

    await expect(replacement).rejects.toMatchObject({ code: 'capture-unavailable' });
    expect(test.capture.start).toHaveBeenCalledOnce();
    await test.service.shutdown();
  });

  it('does not let microphone-test APIs preempt a pending dictation startup', async () => {
    const test = harness();
    const pending = deferred<CaptureStarted>();
    test.capture.start.mockReturnValueOnce(pending.promise);
    const starting = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await vi.waitFor(() => expect(test.capture.start).toHaveBeenCalledOnce());
    const captureId = test.capture.start.mock.calls[0]?.[1] ?? '';

    await expect(
      test.service.startTest(new FakeOwner() as unknown as Electron.WebContents),
    ).resolves.toMatchObject({ status: 'unavailable', reason: 'capture-unavailable' });
    await expect(test.service.stopTest()).resolves.toMatchObject({ status: 'idle' });
    expect(test.capture.stop).not.toHaveBeenCalled();

    pending.resolve({
      captureId,
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
      bindingGeneration: 0,
      systemAudioIncluded: false,
      sampleRate: 16_000,
      channelCount: 1,
    });
    await expect(starting).resolves.toMatchObject({ captureId });
    await test.service.stopDictation(captureId);
    await test.service.shutdown();
  });

  it('still cancels an ownerless pending dictation startup', async () => {
    const test = harness();
    const pending = deferred<CaptureStarted>();
    test.capture.start.mockReturnValueOnce(pending.promise);
    const starting = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop: vi.fn(),
    });
    await vi.waitFor(() => expect(test.capture.start).toHaveBeenCalledOnce());
    const captureId = test.capture.start.mock.calls[0]?.[1] ?? '';

    await test.service.stopDictation();
    expect(test.capture.stop).toHaveBeenCalledWith(captureId);
    pending.resolve({
      captureId,
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
      bindingGeneration: 0,
      systemAudioIncluded: false,
      sampleRate: 16_000,
      channelCount: 1,
    });

    await expect(starting).rejects.toMatchObject({ code: 'capture-unavailable' });
    await test.service.shutdown();
  });

  it('reports an unexpected dictation capture loss to its session owner', async () => {
    const test = harness();
    const onUnexpectedStop = vi.fn();
    const dictation = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop,
    });
    test.capture.stopListener?.(dictation.captureId, 'device-unavailable');
    expect(onUnexpectedStop).toHaveBeenCalledWith('device-unavailable');
    await test.service.shutdown();
  });

  it('does not invalidate microphone evidence when only system audio is lost', async () => {
    const test = harness();
    const invalidateMicrophone = vi.fn();
    const onUnexpectedStop = vi.fn();
    test.service.setWelcomeEvidenceInvalidator(invalidateMicrophone);
    const dictation = await test.service.startDictation(
      { onFrame: vi.fn(), onUnexpectedStop },
      { includeSystemAudio: true },
    );

    test.capture.stopListener?.(dictation.captureId, 'system-audio-unavailable');
    expect(onUnexpectedStop).toHaveBeenCalledWith('system-audio-unavailable');
    expect(invalidateMicrophone).not.toHaveBeenCalled();
    await test.service.shutdown();
  });

  it('does not return a dictation capture that stopped while activation was pending', async () => {
    const test = harness();
    const activation = deferred<undefined>();
    const onUnexpectedStop = vi.fn();
    test.capture.activate.mockReturnValueOnce(activation.promise);

    const starting = test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop,
    });
    await vi.waitFor(() => expect(test.capture.activate).toHaveBeenCalledOnce());
    const captureId = test.capture.activate.mock.calls[0]?.[0] ?? '';
    test.capture.stopListener?.(captureId, 'capture-unavailable');
    activation.resolve(undefined);

    await expect(starting).rejects.toMatchObject({ code: 'capture-unavailable' });
    expect(onUnexpectedStop).toHaveBeenCalledWith('capture-unavailable');
    await test.service.shutdown();
  });

  it('moves to unavailable and releases ownership when the capture port disappears', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.stopListener?.(state.captureId, 'capture-unavailable');
    expect(test.service.getState()).toMatchObject({
      status: 'unavailable',
      reason: 'capture-unavailable',
    });
    expect(owner.listenerCount('destroyed')).toBe(0);
    await test.service.shutdown();
  });
});
