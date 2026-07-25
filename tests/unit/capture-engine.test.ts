import { describe, expect, it, vi } from 'vitest';
import {
  CaptureEngine,
  CaptureEngineError,
  mapCaptureError,
  type CaptureEnvironment,
} from '../../app/src/renderer/capture/capture-engine';

class FakeTrack extends EventTarget {
  readonly stop = vi.fn(() => {
    this.readyState = 'ended';
  });
  readyState: MediaStreamTrackState = 'live';

  constructor(readonly deviceId: string) {
    super();
  }

  getSettings(): MediaTrackSettings {
    return { deviceId: this.deviceId };
  }
}

class FakeWorkletTarget extends EventTarget {
  constructor(private readonly immediateProcessorError: boolean) {
    super();
  }

  override addEventListener(
    type: string,
    callback: EventListenerOrEventListenerObject | null,
    options?: boolean | AddEventListenerOptions,
  ): void {
    super.addEventListener(type, callback, options);
    if (type !== 'processorerror' || !this.immediateProcessorError || callback === null) return;
    const event = new Event('processorerror');
    if (typeof callback === 'function') callback(event);
    else callback.handleEvent(event);
  }
}

class FakePort extends EventTarget {
  readonly start = vi.fn();
  readonly close = vi.fn();
  readonly postMessage = vi.fn((message: unknown) => {
    if (
      typeof message === 'object' &&
      message !== null &&
      (message as Readonly<Record<string, unknown>>).type === 'flush'
    ) {
      this.emit({ type: 'frame', samples: Float32Array.from([0.5, -0.5]), rms: 0.5 });
      this.emit({ type: 'flushed' });
    }
  });

  emit(data: unknown): void {
    this.dispatchEvent(new MessageEvent('message', { data }));
  }
}

function stream(track: FakeTrack): MediaStream {
  return {
    getTracks: () => [track as unknown as MediaStreamTrack],
    getAudioTracks: () => [track as unknown as MediaStreamTrack],
  } as unknown as MediaStream;
}

function mediaDevice(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, label, kind: 'audioinput', groupId: '', toJSON: () => ({}) };
}

function deferred<Value>() {
  let resolvePromise!: (value: Value) => void;
  const promise = new Promise<Value>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

function harness(
  options: {
    readonly devices?: readonly MediaDeviceInfo[];
    readonly enumerateDevices?: () => Promise<MediaDeviceInfo[]>;
    readonly getUserMedia?: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
    readonly sampleRate?: number;
    readonly immediateProcessorError?: boolean;
  } = {},
) {
  const mediaDevices = new EventTarget() as EventTarget & {
    enumerateDevices: () => Promise<MediaDeviceInfo[]>;
    getUserMedia: (constraints: MediaStreamConstraints) => Promise<MediaStream>;
  };
  mediaDevices.enumerateDevices = vi.fn(
    options.enumerateDevices ??
      (() => Promise.resolve([...(options.devices ?? [mediaDevice('default', 'Default')])])),
  );
  const getUserMedia = vi.fn(
    options.getUserMedia ?? (() => Promise.resolve(stream(new FakeTrack('default')))),
  );
  mediaDevices.getUserMedia = getUserMedia;

  const port = new FakePort();
  const sourceConnect = vi.fn();
  const sourceDisconnect = vi.fn();
  const source = {
    connect: sourceConnect,
    disconnect: sourceDisconnect,
  } as unknown as MediaStreamAudioSourceNode;
  const workletConnect = vi.fn();
  const workletDisconnect = vi.fn();
  const worklet = Object.assign(new FakeWorkletTarget(options.immediateProcessorError ?? false), {
    port: port as unknown as MessagePort,
    connect: workletConnect,
    disconnect: workletDisconnect,
  }) as unknown as AudioWorkletNode;
  const contextResume = vi.fn(() => Promise.resolve());
  const contextClose = vi.fn(() => Promise.resolve());
  const context = {
    sampleRate: options.sampleRate ?? 48_000,
    state: 'running',
    audioWorklet: { addModule: vi.fn(() => Promise.resolve()) },
    createMediaStreamSource: vi.fn(() => source),
    destination: {} as AudioDestinationNode,
    resume: contextResume,
    close: contextClose,
  } as unknown as AudioContext;
  let nextTimer = 1;
  const timers = new Map<number, () => void>();
  const frames = vi.fn();
  const devicesChanged = vi.fn();
  const unexpectedStop = vi.fn();
  const environment: CaptureEnvironment = {
    mediaDevices: mediaDevices as unknown as MediaDevices,
    createAudioContext: () => context,
    createWorkletNode: () => worklet,
    workletModuleUrl: 'talking-quill://app/assets/capture.worklet-test.js',
    setTimeout: (callback) => {
      const id = nextTimer;
      nextTimer += 1;
      timers.set(id, callback);
      return id;
    },
    clearTimeout: (id) => timers.delete(id),
  };
  const engine = new CaptureEngine(environment, {
    onDevicesChanged: devicesChanged,
    onFrame: frames,
    onUnexpectedStop: unexpectedStop,
  });
  return {
    engine,
    mediaDevices,
    getUserMedia,
    port,
    worklet,
    sourceDisconnect,
    workletDisconnect,
    contextResume,
    contextClose,
    frames,
    devicesChanged,
    unexpectedStop,
    runTimers: () => {
      const callbacks = [...timers.values()];
      timers.clear();
      for (const callback of callbacks) callback();
    },
  };
}

describe('CaptureEngine', () => {
  it('sorts devices, selects the preferred microphone exactly, and reports the active device', async () => {
    const selectedTrack = new FakeTrack('preferred');
    const test = harness({
      devices: [
        mediaDevice('other', 'Z microphone'),
        mediaDevice('preferred', 'A microphone'),
        mediaDevice('default', 'System default'),
      ],
      getUserMedia: () => Promise.resolve(stream(selectedTrack)),
      sampleRate: 44_100,
    });
    expect((await test.engine.listDevices()).map((device) => device.deviceId)).toEqual([
      'default',
      'preferred',
      'other',
    ]);
    const started = await test.engine.start('preferred');
    expect(started).toMatchObject({
      activeMicrophoneId: 'preferred',
      preferredUnavailable: false,
      sampleRate: 16_000,
      channelCount: 1,
    });
    expect(test.contextResume).not.toHaveBeenCalled();
    await test.engine.activate();
    expect(test.contextResume).toHaveBeenCalledOnce();
    const constraints = test.getUserMedia.mock.calls[0]?.[0];
    expect(constraints?.video).toBe(false);
    expect(constraints?.audio).toEqual(
      expect.objectContaining({ deviceId: { exact: 'preferred' } }),
    );
    await test.engine.stop();
    expect(selectedTrack.stop).toHaveBeenCalledOnce();
  });

  it('retries system default when an enumerated preferred device disappears during acquisition', async () => {
    const fallbackTrack = new FakeTrack('default');
    let acquisition = 0;
    const test = harness({
      devices: [mediaDevice('default', 'Default'), mediaDevice('preferred', 'Preferred')],
      getUserMedia: () => {
        acquisition += 1;
        return acquisition === 1
          ? Promise.reject(new DOMException('disconnected', 'NotReadableError'))
          : Promise.resolve(stream(fallbackTrack));
      },
    });

    await expect(test.engine.start('preferred')).resolves.toMatchObject({
      activeMicrophoneId: 'default',
      preferredUnavailable: true,
    });
    expect(test.getUserMedia).toHaveBeenCalledTimes(2);
    expect(test.getUserMedia.mock.calls[0]?.[0].audio).toEqual(
      expect.objectContaining({ deviceId: { exact: 'preferred' } }),
    );
    expect(test.getUserMedia.mock.calls[1]?.[0].audio).not.toHaveProperty('deviceId');
    await test.engine.stop();
  });

  it('sanitizes, deduplicates, and caps untrusted operating-system device metadata', async () => {
    const devices = [
      mediaDevice('default', ' Default\u0000 microphone '),
      mediaDevice('default', 'Duplicate'),
      mediaDevice('', 'Missing id'),
      mediaDevice('x'.repeat(1_025), 'Oversized id'),
      ...Array.from({ length: 140 }, (_value, index) =>
        mediaDevice(`device-${String(index)}`, `${'L'.repeat(300)}\n${String(index)}`),
      ),
    ];
    const test = harness({ devices });

    const listed = await test.engine.listDevices();

    expect(listed).toHaveLength(128);
    expect(listed[0]).toEqual({
      deviceId: 'default',
      label: 'Default microphone',
      isDefault: true,
    });
    expect(new Set(listed.map((device) => device.deviceId)).size).toBe(listed.length);
    expect(listed.every((device) => device.label.length <= 256)).toBe(true);
    expect(
      listed.every((device) => {
        for (const character of device.label) {
          const code = character.charCodeAt(0);
          if (code <= 31 || code === 127) return false;
        }
        return true;
      }),
    ).toBe(true);
  });

  it('falls back without deleting a missing preference and debounces hot-plug snapshots', async () => {
    let acquisition = 0;
    const test = harness({
      devices: [mediaDevice('default', '')],
      getUserMedia: () => {
        acquisition += 1;
        return acquisition === 1
          ? Promise.reject(new DOMException('missing', 'NotFoundError'))
          : Promise.resolve(stream(new FakeTrack('default')));
      },
    });
    expect(await test.engine.start('disconnected')).toMatchObject({ preferredUnavailable: true });
    await test.engine.activate();
    test.mediaDevices.dispatchEvent(new Event('devicechange'));
    test.mediaDevices.dispatchEvent(new Event('devicechange'));
    expect(test.devicesChanged).not.toHaveBeenCalled();
    test.runTimers();
    await vi.waitFor(() => expect(test.devicesChanged).toHaveBeenCalledOnce());
    await test.engine.stop();
  });

  it('refreshes anonymous enumeration after permission and reports Bluetooth hot-plug labels', async () => {
    let devices = [mediaDevice('', '')];
    const test = harness({ enumerateDevices: () => Promise.resolve(devices) });
    expect(await test.engine.listDevices()).toEqual([]);

    devices = [
      mediaDevice('default', 'System default'),
      mediaDevice('bluetooth-headset', 'Bluetooth Hands-Free AG Audio'),
    ];
    expect(await test.engine.listDevices()).toEqual([
      { deviceId: 'default', label: 'System default', isDefault: true },
      {
        deviceId: 'bluetooth-headset',
        label: 'Bluetooth Hands-Free AG Audio',
        isDefault: false,
      },
    ]);

    test.mediaDevices.dispatchEvent(new Event('devicechange'));
    test.runTimers();
    await vi.waitFor(() =>
      expect(test.devicesChanged).toHaveBeenCalledWith(
        expect.arrayContaining([
          expect.objectContaining({
            deviceId: 'bluetooth-headset',
            label: 'Bluetooth Hands-Free AG Audio',
          }),
        ]),
      ),
    );
  });

  it.each([
    [new DOMException('denied', 'NotAllowedError'), 'permission-denied'],
    [new DOMException('security', 'SecurityError'), 'permission-denied'],
    [new DOMException('missing', 'NotFoundError'), 'no-device'],
    [new DOMException('busy', 'NotReadableError'), 'device-unavailable'],
    [new DOMException('constraints', 'OverconstrainedError'), 'device-unavailable'],
    [new Error('unknown'), 'capture-failed'],
  ] as const)('maps getUserMedia %s without inferring denial from enumeration', (error, code) => {
    expect(mapCaptureError(error)).toBe(code);
  });

  it('forwards the final flushed partial PCM frame before stopping and tears down every resource', async () => {
    const track = new FakeTrack('default');
    const test = harness({ getUserMedia: () => Promise.resolve(stream(track)) });
    await test.engine.start(null);
    await test.engine.activate();
    test.port.emit({ type: 'frame', samples: new Float32Array(320), rms: 0.2 });
    await test.engine.stop();
    expect(test.frames).toHaveBeenCalledTimes(2);
    expect(test.frames).toHaveBeenLastCalledWith(Float32Array.from([0.5, -0.5]), 0.5);
    expect(test.sourceDisconnect).toHaveBeenCalledOnce();
    expect(test.workletDisconnect).toHaveBeenCalledOnce();
    expect(test.port.close).toHaveBeenCalledOnce();
    expect(track.stop).toHaveBeenCalledOnce();
    expect(test.contextClose).toHaveBeenCalledOnce();
  });

  it('stops a late getUserMedia stream after cancellation and never activates it', async () => {
    const pending = deferred<MediaStream>();
    const track = new FakeTrack('late');
    const test = harness({ getUserMedia: () => pending.promise });
    const starting = test.engine.start(null);
    await vi.waitFor(() => expect(test.getUserMedia).toHaveBeenCalledOnce());
    await test.engine.stop();
    pending.resolve(stream(track));
    await expect(starting).rejects.toBeInstanceOf(CaptureEngineError);
    expect(track.stop).toHaveBeenCalledOnce();
    expect(test.contextResume).not.toHaveBeenCalled();
  });

  it('rejects and tears down a stream whose track is already ended during startup', async () => {
    const track = new FakeTrack('already-ended');
    track.readyState = 'ended';
    const test = harness({ getUserMedia: () => Promise.resolve(stream(track)) });
    await expect(test.engine.start(null)).rejects.toMatchObject({ code: 'device-unavailable' });
    expect(track.stop).toHaveBeenCalledOnce();
    expect(test.contextClose).toHaveBeenCalledOnce();
    expect(test.contextResume).not.toHaveBeenCalled();
  });

  it('rejects and tears down a processor error raised during listener registration', async () => {
    const track = new FakeTrack('immediate-processor-error');
    const test = harness({
      getUserMedia: () => Promise.resolve(stream(track)),
      immediateProcessorError: true,
    });
    await expect(test.engine.start(null)).rejects.toMatchObject({ code: 'worklet-unavailable' });
    expect(track.stop).toHaveBeenCalledOnce();
    expect(test.contextClose).toHaveBeenCalledOnce();
    expect(test.contextResume).not.toHaveBeenCalled();
  });

  it('rejects out-of-range worklet samples before forwarding PCM', async () => {
    const test = harness();
    await test.engine.start(null);
    await test.engine.activate();
    test.port.emit({ type: 'frame', samples: Float32Array.from([1.01]), rms: 0.2 });
    test.port.emit({ type: 'frame', samples: Float32Array.from([-1.01]), rms: 0.2 });
    expect(test.frames).not.toHaveBeenCalled();
    await test.engine.stop();
  });

  it('tears down and reports a worklet processor failure', async () => {
    const track = new FakeTrack('processor-error');
    const test = harness({ getUserMedia: () => Promise.resolve(stream(track)) });
    await test.engine.start(null);
    await test.engine.activate();
    test.worklet.dispatchEvent(new Event('processorerror'));
    await vi.waitFor(() => expect(test.unexpectedStop).toHaveBeenCalledWith('error'));
    expect(track.stop).toHaveBeenCalledOnce();
  });

  it('ends and cleans an active graph exactly once when the track is unplugged', async () => {
    const track = new FakeTrack('hotplug');
    const test = harness({ getUserMedia: () => Promise.resolve(stream(track)) });
    await test.engine.start(null);
    await test.engine.activate();
    track.dispatchEvent(new Event('ended'));
    await vi.waitFor(() => expect(test.unexpectedStop).toHaveBeenCalledWith('device-lost'));
    expect(track.stop).toHaveBeenCalledOnce();
    await test.engine.stop();
    expect(track.stop).toHaveBeenCalledOnce();
  });

  it('treats unplug and processor events racing an intentional stop as one teardown', async () => {
    const track = new FakeTrack('stopping');
    const test = harness({ getUserMedia: () => Promise.resolve(stream(track)) });
    await test.engine.start(null);
    await test.engine.activate();
    test.port.postMessage.mockImplementation(() => undefined);

    const stopping = test.engine.stop();
    track.dispatchEvent(new Event('ended'));
    test.worklet.dispatchEvent(new Event('processorerror'));
    test.runTimers();
    await stopping;

    expect(test.unexpectedStop).not.toHaveBeenCalled();
    expect(test.sourceDisconnect).toHaveBeenCalledOnce();
    expect(test.workletDisconnect).toHaveBeenCalledOnce();
    expect(test.port.close).toHaveBeenCalledOnce();
    expect(track.stop).toHaveBeenCalledOnce();
    expect(test.contextClose).toHaveBeenCalledOnce();
  });
});
