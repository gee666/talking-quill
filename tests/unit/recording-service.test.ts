import { EventEmitter } from 'node:events';
import { describe, expect, it, vi } from 'vitest';
import {
  CaptureClientError,
  type CaptureFrame,
  type CaptureStarted,
  type CaptureWindowClient,
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

class FakeCaptureClient {
  readonly attach = vi.fn();
  readonly listDevices = vi.fn(() =>
    Promise.resolve([{ deviceId: 'default', label: 'Default', isDefault: true }]),
  );
  readonly start = vi.fn<(preferred: string | null, captureId: string) => Promise<CaptureStarted>>(
    (_preferred, captureId) =>
      Promise.resolve({
        captureId,
        activeMicrophoneId: 'default',
        preferredUnavailable: false,
        sampleRate: 16_000,
        channelCount: 1,
      }),
  );
  readonly activate = vi.fn<(captureId: string) => Promise<void>>(() => Promise.resolve());
  readonly stop = vi.fn(() => Promise.resolve());
  readonly reset = vi.fn();
  readonly dispose = vi.fn();
  frameListener: ((frame: CaptureFrame) => void) | null = null;
  deviceListener: ((devices: readonly MicrophoneDevice[]) => void) | null = null;
  stopListener: ((captureId: string) => void) | null = null;

  onFrame(listener: (frame: CaptureFrame) => void): () => void {
    this.frameListener = listener;
    return () => {
      this.frameListener = null;
    };
  }

  onDevicesChanged(listener: (devices: readonly MicrophoneDevice[]) => void): () => void {
    this.deviceListener = listener;
    return () => {
      this.deviceListener = null;
    };
  }

  onUnexpectedStop(listener: (captureId: string) => void): () => void {
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
  } = {},
) {
  const capture = new FakeCaptureClient();
  const settings = {
    get: () => ({
      ...structuredClone(DEFAULT_SETTINGS),
      recording: {
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
  const service = new RecordingService(
    capture as unknown as CaptureWindowClient,
    settings as unknown as SettingsStore,
    events as unknown as IpcEventEmitter,
    permission,
  );
  const captureWebContents = {
    id: 7,
    isDestroyed: () => false,
    reload: vi.fn(),
  };
  service.attachCapture(captureWebContents as unknown as Electron.WebContents);
  return { capture, captureWebContents, events, permission, service };
}

describe('RecordingService ownership', () => {
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
    await test.service.getDevices();
    test.capture.deviceListener?.([]);
    test.capture.listDevices.mockResolvedValueOnce([]);

    await expect(test.service.getDevices()).resolves.toMatchObject({ devices: [bluetooth] });
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
    test.capture.deviceListener?.([
      { deviceId: 'stale-event', label: 'Stale event', isDefault: false },
    ]);
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
    expect(test.permission.allowsCheck(permissionRequest)).toBe(true);
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

  it('reports an unexpected dictation capture loss to its session owner', async () => {
    const test = harness();
    const onUnexpectedStop = vi.fn();
    const dictation = await test.service.startDictation({
      onFrame: vi.fn(),
      onUnexpectedStop,
    });
    test.capture.stopListener?.(dictation.captureId);
    expect(onUnexpectedStop).toHaveBeenCalledWith('device-unavailable');
    await test.service.shutdown();
  });

  it('moves to unavailable and releases ownership when the capture port disappears', async () => {
    const test = harness();
    const owner = new FakeOwner();
    const state = await test.service.startTest(owner as unknown as Electron.WebContents);
    if (state.status !== 'active') throw new Error('Expected an active test');
    test.capture.stopListener?.(state.captureId);
    expect(test.service.getState()).toMatchObject({
      status: 'unavailable',
      reason: 'device-unavailable',
    });
    expect(owner.listenerCount('destroyed')).toBe(0);
    await test.service.shutdown();
  });
});
