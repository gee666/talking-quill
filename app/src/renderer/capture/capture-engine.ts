import {
  DEVICE_CHANGE_DEBOUNCE_MS,
  MAX_MICROPHONE_DEVICES,
  MAX_MICROPHONE_ID_LENGTH,
  MAX_MICROPHONE_LABEL_LENGTH,
  PCM_CHANNEL_COUNT,
  PCM_FRAME_SAMPLES,
  PCM_SAMPLE_RATE,
} from '../../shared/constants/audio';
import type { MicrophoneDevice } from '../../shared/schemas/audio';

export type CaptureFailureCode =
  | 'permission-denied'
  | 'no-device'
  | 'device-unavailable'
  | 'unsupported-audio-format'
  | 'worklet-unavailable'
  | 'capture-failed';

export interface CaptureStartResult {
  readonly activeMicrophoneId: string | null;
  readonly preferredUnavailable: boolean;
  readonly sampleRate: typeof PCM_SAMPLE_RATE;
  readonly channelCount: typeof PCM_CHANNEL_COUNT;
}

export interface CaptureEngineCallbacks {
  readonly onDevicesChanged: (devices: readonly MicrophoneDevice[]) => void;
  readonly onFrame: (samples: Float32Array, rms: number) => void;
  readonly onUnexpectedStop: (reason: 'device-lost' | 'error') => void;
}

export interface CaptureEnvironment {
  readonly mediaDevices: MediaDevices;
  readonly createAudioContext: () => AudioContext;
  readonly createWorkletNode: (context: AudioContext) => AudioWorkletNode;
  readonly workletModuleUrl: string;
  readonly setTimeout: (callback: () => void, delay: number) => number;
  readonly clearTimeout: (timer: number) => void;
}

interface ActiveCapture {
  readonly generation: number;
  readonly stream: MediaStream;
  readonly context: AudioContext;
  readonly source: MediaStreamAudioSourceNode;
  readonly worklet: AudioWorkletNode;
  readonly onWorkletMessage: (event: MessageEvent<unknown>) => void;
  readonly onTrackEnded: () => void;
  readonly onProcessorError: () => void;
  phase: 'prepared' | 'activating' | 'active';
  startupFailure: 'device-lost' | 'error' | null;
  connected: boolean;
  flushResolver: (() => void) | null;
  teardownPromise: Promise<void> | null;
  releasePromise: Promise<void> | null;
}

export class CaptureEngineError extends Error {
  readonly code: CaptureFailureCode;

  constructor(code: CaptureFailureCode) {
    super(code);
    this.name = 'CaptureEngineError';
    this.code = code;
  }
}

export class CaptureEngine {
  readonly #environment: CaptureEnvironment;
  readonly #callbacks: CaptureEngineCallbacks;
  #active: ActiveCapture | null = null;
  #generation = 0;
  #deviceTimer: number | null = null;
  #disposed = false;

  constructor(environment: CaptureEnvironment, callbacks: CaptureEngineCallbacks) {
    this.#environment = environment;
    this.#callbacks = callbacks;
    this.#environment.mediaDevices.addEventListener('devicechange', this.#onDeviceChange);
  }

  async listDevices(): Promise<readonly MicrophoneDevice[]> {
    const devices = await this.#environment.mediaDevices.enumerateDevices();
    const sanitized = new Map<string, MicrophoneDevice>();
    let anonymousIndex = 0;
    for (const device of devices) {
      if (device.kind !== 'audioinput') continue;
      const deviceId = sanitizeDeviceId(device.deviceId);
      if (deviceId === null || sanitized.has(deviceId)) continue;
      anonymousIndex += 1;
      sanitized.set(deviceId, {
        deviceId,
        label: sanitizeDeviceLabel(device.label, anonymousIndex),
        isDefault: deviceId === 'default',
      });
    }
    return [...sanitized.values()]
      .sort((first, second) => {
        if (first.isDefault !== second.isDefault) return first.isDefault ? -1 : 1;
        return (
          first.label.localeCompare(second.label) || first.deviceId.localeCompare(second.deviceId)
        );
      })
      .slice(0, MAX_MICROPHONE_DEVICES);
  }

  async start(preferredMicrophoneId: string | null): Promise<CaptureStartResult> {
    if (this.#disposed) throw new CaptureEngineError('capture-failed');
    const generation = ++this.#generation;
    await this.#teardownActive(false);
    if (generation !== this.#generation) throw new CaptureEngineError('capture-failed');
    let preferredUnavailable = false;
    let acquisitionDeviceId = preferredMicrophoneId;

    let stream: MediaStream;
    try {
      stream = await this.#acquireStream(preferredMicrophoneId);
    } catch (error: unknown) {
      if (generation !== this.#generation) throw new CaptureEngineError('capture-failed');
      if (preferredMicrophoneId === null || !isPreferredDeviceRace(error)) {
        throw new CaptureEngineError(mapCaptureError(error));
      }
      preferredUnavailable = true;
      acquisitionDeviceId = null;
      try {
        stream = await this.#acquireStream(null);
      } catch (fallbackError: unknown) {
        throw new CaptureEngineError(mapCaptureError(fallbackError));
      }
    }

    if (generation !== this.#generation) {
      stopStream(stream);
      throw new CaptureEngineError('capture-failed');
    }

    let context: AudioContext | null = null;
    let source: MediaStreamAudioSourceNode | null = null;
    let worklet: AudioWorkletNode | null = null;
    let activeInstalled = false;
    try {
      context = this.#environment.createAudioContext();
      if (context.sampleRate < PCM_SAMPLE_RATE) {
        throw new CaptureEngineError('unsupported-audio-format');
      }
      await context.audioWorklet.addModule(this.#environment.workletModuleUrl);
      if (generation !== this.#generation) throw new Error('stale capture');
      source = context.createMediaStreamSource(stream);
      worklet = this.#environment.createWorkletNode(context);
      const active: ActiveCapture = {
        generation,
        stream,
        context,
        source,
        worklet,
        onWorkletMessage: (event) => this.#handleWorkletMessage(event),
        onTrackEnded: () => this.#handleCaptureFailure(generation, 'device-lost'),
        onProcessorError: () => this.#handleCaptureFailure(generation, 'error'),
        phase: 'prepared',
        startupFailure: null,
        connected: false,
        flushResolver: null,
        teardownPromise: null,
        releasePromise: null,
      };
      this.#active = active;
      activeInstalled = true;
      worklet.port.addEventListener('message', active.onWorkletMessage);
      worklet.addEventListener('processorerror', active.onProcessorError);
      worklet.port.start();
      for (const track of stream.getTracks()) track.addEventListener('ended', active.onTrackEnded);
      if (stream.getTracks().some((track) => track.readyState === 'ended')) {
        active.startupFailure = 'device-lost';
      }
      if (active.startupFailure !== null || generation !== this.#generation) {
        const reason = active.startupFailure;
        ++this.#generation;
        await this.#teardownActive(false);
        throw new CaptureEngineError(
          reason === 'device-lost' ? 'device-unavailable' : 'worklet-unavailable',
        );
      }
    } catch (error: unknown) {
      if (this.#active?.generation === generation) {
        ++this.#generation;
        await this.#teardownActive(false);
      } else if (!activeInstalled) {
        source?.disconnect();
        worklet?.disconnect();
        worklet?.port.close();
        stopStream(stream);
        if (context !== null) await context.close().catch(() => undefined);
      }
      if (error instanceof CaptureEngineError) throw error;
      throw new CaptureEngineError('worklet-unavailable');
    }

    const activeTrack = stream.getAudioTracks()[0];
    const activeDeviceId = activeTrack?.getSettings().deviceId;
    const reportedDeviceId = activeDeviceId === undefined ? null : sanitizeDeviceId(activeDeviceId);
    return {
      activeMicrophoneId: reportedDeviceId ?? acquisitionDeviceId,
      preferredUnavailable,
      sampleRate: PCM_SAMPLE_RATE,
      channelCount: PCM_CHANNEL_COUNT,
    };
  }

  #acquireStream(deviceId: string | null): Promise<MediaStream> {
    return this.#environment.mediaDevices.getUserMedia({
      audio: {
        ...(deviceId === null ? {} : { deviceId: { exact: deviceId } }),
        channelCount: { ideal: 1 },
        sampleRate: { ideal: 48_000 },
        echoCancellation: { ideal: false },
        noiseSuppression: { ideal: false },
        autoGainControl: { ideal: false },
      },
      video: false,
    });
  }

  async activate(): Promise<void> {
    const active = this.#active;
    if (active?.phase !== 'prepared') {
      throw new CaptureEngineError('capture-failed');
    }
    active.phase = 'activating';
    try {
      active.source.connect(active.worklet);
      active.worklet.connect(active.context.destination);
      active.connected = true;
      await active.context.resume();
      if (active.context.state !== 'running') {
        throw new CaptureEngineError('worklet-unavailable');
      }
      if (
        this.#active !== active ||
        active.generation !== this.#generation ||
        active.startupFailure !== null
      ) {
        throw new CaptureEngineError(
          active.startupFailure === 'device-lost' ? 'device-unavailable' : 'worklet-unavailable',
        );
      }
      active.phase = 'active';
    } catch (error: unknown) {
      if (this.#active === active) {
        ++this.#generation;
        await this.#teardownActive(false);
      }
      if (error instanceof CaptureEngineError) throw error;
      throw new CaptureEngineError('worklet-unavailable');
    }
  }

  async stop(): Promise<void> {
    ++this.#generation;
    await this.#teardownActive(true);
  }

  disposeImmediately(): void {
    if (this.#disposed) return;
    this.#disposed = true;
    ++this.#generation;
    if (this.#deviceTimer !== null) this.#environment.clearTimeout(this.#deviceTimer);
    this.#deviceTimer = null;
    this.#environment.mediaDevices.removeEventListener('devicechange', this.#onDeviceChange);
    const active = this.#active;
    this.#active = null;
    if (active === null) return;
    active.flushResolver?.();
    void this.#releaseActive(active);
  }

  async #teardownActive(flush: boolean): Promise<void> {
    const active = this.#active;
    if (active === null) return;
    active.teardownPromise ??= (async () => {
      if (flush) await this.#flush(active);
      if (this.#active === active) this.#active = null;
      await this.#releaseActive(active);
    })();
    await active.teardownPromise;
  }

  #releaseActive(active: ActiveCapture): Promise<void> {
    active.releasePromise ??= (() => {
      this.#removeActiveListeners(active);
      active.source.disconnect();
      active.worklet.disconnect();
      active.worklet.port.close();
      stopStream(active.stream);
      return active.context.close().catch(() => undefined);
    })();
    return active.releasePromise;
  }

  #removeActiveListeners(active: ActiveCapture): void {
    active.worklet.port.removeEventListener('message', active.onWorkletMessage);
    active.worklet.removeEventListener('processorerror', active.onProcessorError);
    for (const track of active.stream.getTracks()) {
      track.removeEventListener('ended', active.onTrackEnded);
    }
  }

  #flush(active: ActiveCapture): Promise<void> {
    return new Promise((resolve) => {
      let finished = false;
      let timer = 0;
      const onMessage = (event: MessageEvent<unknown>) => {
        if (
          typeof event.data === 'object' &&
          event.data !== null &&
          Reflect.get(event.data, 'type') === 'flushed'
        ) {
          finish();
        }
      };
      const finish = () => {
        if (finished) return;
        finished = true;
        this.#environment.clearTimeout(timer);
        active.worklet.port.removeEventListener('message', onMessage);
        active.flushResolver = null;
        resolve();
      };
      active.worklet.port.addEventListener('message', onMessage);
      active.flushResolver = finish;
      timer = this.#environment.setTimeout(finish, 100);
      active.worklet.port.postMessage({ type: 'flush' });
    });
  }

  #handleWorkletMessage(event: MessageEvent<unknown>): void {
    const active = this.#active;
    if (active === null) return;
    const value = event.data;
    if (typeof value !== 'object' || value === null) return;
    const record = value as Readonly<Record<string, unknown>>;
    const type = record.type;
    if (type === 'flushed') {
      active.flushResolver?.();
      return;
    }
    if (type !== 'frame') return;
    const samples = record.samples;
    const rms = record.rms;
    if (
      !(samples instanceof Float32Array) ||
      samples.length === 0 ||
      samples.length > PCM_FRAME_SAMPLES ||
      !hasNormalizedSamples(samples) ||
      typeof rms !== 'number' ||
      !Number.isFinite(rms) ||
      rms < 0 ||
      rms > 1
    ) {
      return;
    }
    this.#callbacks.onFrame(samples, rms);
  }

  #handleCaptureFailure(generation: number, reason: 'device-lost' | 'error'): void {
    const active = this.#active;
    if (
      active?.generation !== generation ||
      generation !== this.#generation ||
      active.teardownPromise !== null
    ) {
      return;
    }
    active.startupFailure = reason;
    if (active.phase !== 'active') return;
    ++this.#generation;
    void this.#teardownActive(false).finally(() => this.#callbacks.onUnexpectedStop(reason));
  }

  readonly #onDeviceChange = () => {
    if (this.#disposed) return;
    if (this.#deviceTimer !== null) this.#environment.clearTimeout(this.#deviceTimer);
    this.#deviceTimer = this.#environment.setTimeout(() => {
      this.#deviceTimer = null;
      void this.listDevices().then(
        (devices) => this.#callbacks.onDevicesChanged(devices),
        () => this.#callbacks.onDevicesChanged([]),
      );
    }, DEVICE_CHANGE_DEBOUNCE_MS);
  };
}

export function createBrowserCaptureEnvironment(workletModuleUrl: string): CaptureEnvironment {
  return {
    mediaDevices: navigator.mediaDevices,
    createAudioContext: () => new AudioContext({ latencyHint: 'interactive' }),
    createWorkletNode: (context) =>
      new AudioWorkletNode(context, 'talking-quill-capture', {
        numberOfInputs: 1,
        numberOfOutputs: 1,
        outputChannelCount: [1],
        channelCountMode: 'max',
      }),
    workletModuleUrl,
    setTimeout: (callback, delay) => window.setTimeout(callback, delay),
    clearTimeout: (timer) => window.clearTimeout(timer),
  };
}

function stopStream(stream: MediaStream): void {
  for (const track of stream.getTracks()) track.stop();
}

function sanitizeDeviceId(deviceId: string): string | null {
  if (
    deviceId.trim().length === 0 ||
    deviceId.length > MAX_MICROPHONE_ID_LENGTH ||
    hasControlCharacter(deviceId)
  ) {
    return null;
  }
  return deviceId;
}

function sanitizeDeviceLabel(label: string, anonymousIndex: number): string {
  let cleaned = '';
  for (const character of label) {
    cleaned += isControlCharacter(character) ? ' ' : character;
  }
  const sanitized = cleaned
    .replace(/\s+/gu, ' ')
    .trim()
    .slice(0, MAX_MICROPHONE_LABEL_LENGTH)
    .trim();
  return sanitized || `Microphone ${String(anonymousIndex)}`;
}

function hasControlCharacter(value: string): boolean {
  for (const character of value) {
    if (isControlCharacter(character)) return true;
  }
  return false;
}

function isControlCharacter(value: string): boolean {
  const code = value.charCodeAt(0);
  return code <= 31 || code === 127;
}

function hasNormalizedSamples(samples: Float32Array): boolean {
  for (const sample of samples) {
    if (!Number.isFinite(sample) || sample < -1 || sample > 1) return false;
  }
  return true;
}

function isPreferredDeviceRace(error: unknown): boolean {
  return (
    error instanceof DOMException &&
    (error.name === 'NotFoundError' ||
      error.name === 'NotReadableError' ||
      error.name === 'OverconstrainedError')
  );
}

export function mapCaptureError(error: unknown): CaptureFailureCode {
  if (error instanceof DOMException) {
    if (error.name === 'NotAllowedError' || error.name === 'SecurityError') {
      return 'permission-denied';
    }
    if (error.name === 'NotFoundError') return 'no-device';
    if (error.name === 'NotReadableError' || error.name === 'OverconstrainedError') {
      return 'device-unavailable';
    }
  }
  return 'capture-failed';
}
