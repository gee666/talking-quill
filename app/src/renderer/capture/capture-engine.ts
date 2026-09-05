import {
  CAPTURE_WORKLET_FLUSH_TIMEOUT_MS,
  CAPTURE_WORKLET_PROCESSOR_NAME,
  DEVICE_CHANGE_DEBOUNCE_MS,
  PCM_CHANNEL_COUNT,
  PCM_SAMPLE_RATE,
} from '../../shared/constants/audio';
import type { MicrophoneDevice } from '../../shared/schemas/audio';

import { sanitizeDeviceId, sanitizeMicrophoneDevices } from './capture-devices';
import {
  allowsDefaultFallback,
  captureFailureCode,
  CaptureEngineError,
  mapCaptureError,
  type CaptureStopReason,
} from './capture-errors';
import { readCaptureWorkletMessage } from './capture-worklet-message';

export { CaptureEngineError, mapCaptureError } from './capture-errors';
export type { CaptureFailureCode, CaptureStopReason } from './capture-errors';

export interface CaptureStartResult {
  readonly activeMicrophoneId: string | null;
  readonly preferredUnavailable: boolean;
  readonly bindingGeneration: number;
  readonly systemAudioIncluded: boolean;
  readonly sampleRate: typeof PCM_SAMPLE_RATE;
  readonly channelCount: typeof PCM_CHANNEL_COUNT;
}

export interface CaptureRebindResult {
  readonly activeMicrophoneId: string | null;
  readonly bindingGeneration: number;
}

export interface CaptureEngineCallbacks {
  readonly onDevicesChanged: (defaultInvalidated: boolean) => void;
  readonly onDefaultInvalidated: (bindingGeneration: number) => void;
  readonly onFrame: (samples: Float32Array, rms: number) => void;
  readonly onUnexpectedStop: (reason: CaptureStopReason) => void;
}

export interface CaptureEnvironment {
  readonly mediaDevices: MediaDevices;
  readonly createAudioContext: () => AudioContext;
  readonly createWorkletNode: (context: AudioContext) => AudioWorkletNode;
  readonly workletModuleUrl: string;
  readonly setTimeout: (callback: () => void, delay: number) => number;
  readonly clearTimeout: (timer: number) => void;
}

interface MicrophoneBinding {
  readonly generation: number;
  readonly stream: MediaStream;
  readonly source: MediaStreamAudioSourceNode;
  readonly onTrackEnded: () => void;
}

interface ActiveCapture {
  readonly generation: number;
  readonly followsSystemDefault: boolean;
  readonly systemStream: MediaStream | null;
  readonly context: AudioContext;
  readonly systemSource: MediaStreamAudioSourceNode | null;
  readonly worklet: AudioWorkletNode;
  readonly onWorkletMessage: (event: MessageEvent<unknown>) => void;
  readonly onSystemTrackEnded: () => void;
  readonly onProcessorError: () => void;
  microphone: MicrophoneBinding;
  phase: 'prepared' | 'activating' | 'active';
  startupFailure: CaptureStopReason | null;
  defaultTrackEnded: boolean;
  defaultChangePending: boolean;
  deviceChangeReported: boolean;
  suppressNextDeviceChange: boolean;
  connected: boolean;
  flushResolver: (() => void) | null;
  teardownPromise: Promise<void> | null;
  releasePromise: Promise<void> | null;
  rebindPromise: Promise<CaptureRebindResult> | null;
}

export class CaptureEngine {
  readonly #environment: CaptureEnvironment;
  readonly #callbacks: CaptureEngineCallbacks;
  #active: ActiveCapture | null = null;
  #startupSettled: Promise<void> | null = null;
  #generation = 0;
  #deviceTimer: number | null = null;
  #deviceChangeIncludedDefaultInvalidation = false;
  #startingDefaultGeneration: number | null = null;
  #defaultChangedDuringStartup = false;
  #disposed = false;

  constructor(environment: CaptureEnvironment, callbacks: CaptureEngineCallbacks) {
    this.#environment = environment;
    this.#callbacks = callbacks;
    this.#environment.mediaDevices.addEventListener('devicechange', this.#onDeviceChange);
  }

  async listDevices(): Promise<readonly MicrophoneDevice[]> {
    const devices = await this.#environment.mediaDevices.enumerateDevices();
    return sanitizeMicrophoneDevices(devices);
  }

  start(
    preferredMicrophoneId: string | null,
    includeSystemAudio = false,
  ): Promise<CaptureStartResult> {
    const startup = this.#start(preferredMicrophoneId, includeSystemAudio);
    const settled = startup.then(
      () => undefined,
      () => undefined,
    );
    this.#startupSettled = settled;
    void settled.then(() => {
      if (this.#startupSettled === settled) this.#startupSettled = null;
    });
    return startup;
  }

  async #start(
    preferredMicrophoneId: string | null,
    includeSystemAudio: boolean,
  ): Promise<CaptureStartResult> {
    if (this.#disposed) throw new CaptureEngineError('capture-failed');
    const generation = ++this.#generation;
    await this.#teardownActive(false);
    if (generation !== this.#generation) throw new CaptureEngineError('capture-failed');
    const preferredDeviceId =
      preferredMicrophoneId === null ? null : sanitizeDeviceId(preferredMicrophoneId);
    if (preferredMicrophoneId !== null && preferredDeviceId === null) {
      throw new CaptureEngineError('device-unavailable');
    }
    this.#startingDefaultGeneration = preferredMicrophoneId === null ? generation : null;
    this.#defaultChangedDuringStartup = false;

    try {
      let stream: MediaStream;
      let preferredUnavailable = false;
      try {
        stream = await this.#acquireStream(preferredDeviceId);
      } catch (error: unknown) {
        if (generation !== this.#generation) throw new CaptureEngineError('capture-failed');
        if (preferredDeviceId === null || !allowsDefaultFallback(error)) {
          const code = mapCaptureError(error);
          throw new CaptureEngineError(
            preferredDeviceId !== null && code === 'no-device' ? 'device-unavailable' : code,
          );
        }
        preferredUnavailable = true;
        this.#startingDefaultGeneration = generation;
        try {
          stream = await this.#acquireStream(null);
        } catch {
          if (generation !== this.#generation) throw new CaptureEngineError('capture-failed');
          throw new CaptureEngineError('device-unavailable');
        }
      }

      if (generation !== this.#generation) {
        stopStream(stream);
        throw new CaptureEngineError('capture-failed');
      }
      const initialTrack = liveAudioTrack(stream);
      if (initialTrack === null) {
        stopStream(stream);
        throw new CaptureEngineError('device-unavailable');
      }
      const reportedDeviceId = sanitizeDeviceId(initialTrack.getSettings().deviceId);
      if (
        preferredDeviceId !== null &&
        !preferredUnavailable &&
        reportedDeviceId !== null &&
        reportedDeviceId !== preferredDeviceId
      ) {
        stopStream(stream);
        throw new CaptureEngineError('device-unavailable');
      }

      let systemStream: MediaStream | null = null;
      if (includeSystemAudio) {
        try {
          systemStream = await this.#acquireSystemStream();
        } catch {
          stopStream(stream);
          throw new CaptureEngineError('system-audio-unavailable');
        }
        if (generation !== this.#generation) {
          stopStream(stream);
          stopStream(systemStream);
          throw new CaptureEngineError('capture-failed');
        }
      }

      let context: AudioContext | null = null;
      let source: MediaStreamAudioSourceNode | null = null;
      let systemSource: MediaStreamAudioSourceNode | null = null;
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
        systemSource = systemStream === null ? null : context.createMediaStreamSource(systemStream);
        worklet = this.#environment.createWorkletNode(context);
        const microphone: MicrophoneBinding = {
          generation: 0,
          stream,
          source,
          onTrackEnded: () => this.#handleMicrophoneTrackEnded(generation, microphone),
        };
        const active: ActiveCapture = {
          generation,
          followsSystemDefault: preferredMicrophoneId === null || preferredUnavailable,
          systemStream,
          context,
          systemSource,
          worklet,
          microphone,
          onWorkletMessage: (event) => this.#handleWorkletMessage(event),
          onSystemTrackEnded: () => this.#handleCaptureFailure(generation, 'system-audio-lost'),
          onProcessorError: () => this.#handleCaptureFailure(generation, 'error'),
          phase: 'prepared',
          startupFailure: null,
          defaultTrackEnded: false,
          defaultChangePending:
            this.#startingDefaultGeneration === generation && this.#defaultChangedDuringStartup,
          deviceChangeReported: false,
          suppressNextDeviceChange: false,
          connected: false,
          flushResolver: null,
          teardownPromise: null,
          releasePromise: null,
          rebindPromise: null,
        };
        this.#active = active;
        this.#startingDefaultGeneration = null;
        this.#defaultChangedDuringStartup = false;
        activeInstalled = true;
        worklet.port.addEventListener('message', active.onWorkletMessage);
        worklet.addEventListener('processorerror', active.onProcessorError);
        worklet.port.start();
        this.#addMicrophoneListeners(microphone);
        for (const track of active.systemStream?.getTracks() ?? []) {
          track.addEventListener('ended', active.onSystemTrackEnded);
        }
        if (liveAudioTrack(active.microphone.stream) === null) {
          active.startupFailure = 'device-lost';
        } else if (
          active.systemStream?.getTracks().some((track) => track.readyState === 'ended') === true
        ) {
          active.startupFailure = 'system-audio-lost';
        }
        if (active.startupFailure !== null || generation !== this.#generation) {
          const reason = active.startupFailure;
          ++this.#generation;
          await this.#teardownActive(false);
          throw new CaptureEngineError(captureFailureCode(reason));
        }
      } catch (error: unknown) {
        if (this.#active?.generation === generation) {
          ++this.#generation;
          await this.#teardownActive(false);
        } else if (!activeInstalled) {
          source?.disconnect();
          systemSource?.disconnect();
          worklet?.disconnect();
          worklet?.port.close();
          stopStream(stream);
          if (systemStream !== null) stopStream(systemStream);
          if (context !== null) await context.close().catch(() => undefined);
        }
        if (error instanceof CaptureEngineError) throw error;
        throw new CaptureEngineError('worklet-unavailable');
      }

      return {
        activeMicrophoneId: reportedDeviceId ?? (preferredUnavailable ? null : preferredDeviceId),
        preferredUnavailable,
        bindingGeneration: 0,
        systemAudioIncluded: systemStream !== null,
        sampleRate: PCM_SAMPLE_RATE,
        channelCount: PCM_CHANNEL_COUNT,
      };
    } finally {
      if (this.#startingDefaultGeneration === generation) {
        this.#startingDefaultGeneration = null;
        this.#defaultChangedDuringStartup = false;
      }
    }
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

  async #acquireSystemStream(): Promise<MediaStream> {
    const stream = await this.#environment.mediaDevices.getDisplayMedia({
      audio: true,
      video: true,
    });
    const audioTracks = stream.getAudioTracks();
    if (audioTracks.length === 0 || audioTracks.every((track) => track.readyState === 'ended')) {
      stopStream(stream);
      throw new CaptureEngineError('system-audio-unavailable');
    }
    for (const track of stream.getVideoTracks()) {
      stream.removeTrack(track);
      track.stop();
    }
    return stream;
  }

  async activate(): Promise<void> {
    const active = this.#active;
    if (active?.phase !== 'prepared') {
      throw new CaptureEngineError('capture-failed');
    }
    active.phase = 'activating';
    try {
      active.microphone.source.connect(active.worklet, 0, 0);
      active.systemSource?.connect(active.worklet, 0, 1);
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
        throw new CaptureEngineError(captureFailureCode(active.startupFailure));
      }
      active.phase = 'active';
      if (active.defaultChangePending) {
        active.defaultChangePending = false;
        this.#notifyDefaultInvalidated(active.microphone.generation);
      }
    } catch (error: unknown) {
      if (this.#active === active) {
        ++this.#generation;
        await this.#teardownActive(false);
      }
      if (error instanceof CaptureEngineError) throw error;
      throw new CaptureEngineError('worklet-unavailable');
    }
  }

  rebindDefault(bindingGeneration: number): Promise<CaptureRebindResult> {
    const active = this.#active;
    if (
      active === null ||
      !active.followsSystemDefault ||
      active.phase !== 'active' ||
      active.teardownPromise !== null ||
      active.microphone.generation !== bindingGeneration
    ) {
      return Promise.reject(new CaptureEngineError('capture-failed'));
    }
    if (active.rebindPromise !== null) return active.rebindPromise;
    const rebind = this.#rebindDefault(active, bindingGeneration);
    active.rebindPromise = rebind;
    void rebind.then(
      () => {
        if (active.rebindPromise === rebind) active.rebindPromise = null;
      },
      () => {
        if (active.rebindPromise === rebind) active.rebindPromise = null;
      },
    );
    return rebind;
  }

  async #rebindDefault(
    active: ActiveCapture,
    bindingGeneration: number,
  ): Promise<CaptureRebindResult> {
    let stream: MediaStream;
    try {
      stream = await this.#acquireStream(null);
    } catch (error: unknown) {
      throw new CaptureEngineError(mapCaptureError(error));
    }
    const track = liveAudioTrack(stream);
    if (track === null) {
      stopStream(stream);
      throw new CaptureEngineError('device-unavailable');
    }
    if (!this.#canCommitRebind(active, bindingGeneration)) {
      stopStream(stream);
      throw new CaptureEngineError('capture-failed');
    }

    let source: MediaStreamAudioSourceNode;
    try {
      source = active.context.createMediaStreamSource(stream);
    } catch {
      stopStream(stream);
      throw new CaptureEngineError('capture-failed');
    }
    const replacement: MicrophoneBinding = {
      generation: bindingGeneration + 1,
      stream,
      source,
      onTrackEnded: () => this.#handleMicrophoneTrackEnded(active.generation, replacement),
    };
    try {
      source.connect(active.worklet, 0, 0);
    } catch {
      source.disconnect();
      stopStream(stream);
      throw new CaptureEngineError('capture-failed');
    }
    if (!this.#canCommitRebind(active, bindingGeneration) || liveAudioTrack(stream) === null) {
      source.disconnect();
      stopStream(stream);
      throw new CaptureEngineError('device-unavailable');
    }

    this.#addMicrophoneListeners(replacement);
    const retired = active.microphone;
    active.microphone = replacement;
    active.defaultTrackEnded = false;
    active.defaultChangePending = false;
    active.deviceChangeReported = false;
    active.suppressNextDeviceChange = false;
    this.#removeMicrophoneListeners(retired);
    retired.source.disconnect();
    stopStream(retired.stream);
    return {
      activeMicrophoneId: sanitizeDeviceId(track.getSettings().deviceId),
      bindingGeneration: replacement.generation,
    };
  }

  #canCommitRebind(active: ActiveCapture, bindingGeneration: number): boolean {
    return (
      !this.#disposed &&
      this.#active === active &&
      active.generation === this.#generation &&
      active.phase === 'active' &&
      active.teardownPromise === null &&
      active.microphone.generation === bindingGeneration
    );
  }

  async stop(): Promise<void> {
    const startupSettled = this.#startupSettled;
    const rebindSettled = this.#active?.rebindPromise?.then(
      () => undefined,
      () => undefined,
    );
    ++this.#generation;
    await this.#teardownActive(true);
    await Promise.all([startupSettled, rebindSettled]);
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
      if (flush && active.connected) await this.#flush(active);
      if (this.#active === active) this.#active = null;
      await this.#releaseActive(active);
    })();
    await active.teardownPromise;
  }

  #releaseActive(active: ActiveCapture): Promise<void> {
    active.releasePromise ??= (() => {
      this.#removeActiveListeners(active);
      active.microphone.source.disconnect();
      active.systemSource?.disconnect();
      active.worklet.disconnect();
      active.worklet.port.close();
      stopStream(active.microphone.stream);
      if (active.systemStream !== null) stopStream(active.systemStream);
      // Chromium has already lost every input and worklet reference at this point. Some native
      // audio drivers never settle AudioContext.close(), so it cannot be the capture IPC ack edge.
      void active.context.close().catch(() => undefined);
      return Promise.resolve();
    })();
    return active.releasePromise;
  }

  #addMicrophoneListeners(binding: MicrophoneBinding): void {
    for (const track of binding.stream.getTracks()) {
      track.addEventListener('ended', binding.onTrackEnded);
    }
  }

  #removeMicrophoneListeners(binding: MicrophoneBinding): void {
    for (const track of binding.stream.getTracks()) {
      track.removeEventListener('ended', binding.onTrackEnded);
    }
  }

  #removeActiveListeners(active: ActiveCapture): void {
    active.worklet.port.removeEventListener('message', active.onWorkletMessage);
    active.worklet.removeEventListener('processorerror', active.onProcessorError);
    this.#removeMicrophoneListeners(active.microphone);
    for (const track of active.systemStream?.getTracks() ?? []) {
      track.removeEventListener('ended', active.onSystemTrackEnded);
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
      timer = this.#environment.setTimeout(finish, CAPTURE_WORKLET_FLUSH_TIMEOUT_MS);
      active.worklet.port.postMessage({ type: 'flush' });
    });
  }

  #handleWorkletMessage(event: MessageEvent<unknown>): void {
    const active = this.#active;
    if (active === null) return;
    const message = readCaptureWorkletMessage(event.data);
    if (message === null) return;
    if (message.type === 'flushed') {
      active.flushResolver?.();
      return;
    }
    this.#callbacks.onFrame(message.samples, message.rms);
  }

  #handleMicrophoneTrackEnded(generation: number, binding: MicrophoneBinding): void {
    const active = this.#active;
    if (active?.generation !== generation || active.microphone !== binding) return;
    if (!active.followsSystemDefault) {
      this.#handleCaptureFailure(generation, 'device-lost');
      return;
    }
    if (active.defaultTrackEnded) return;
    active.defaultTrackEnded = true;
    if (active.phase !== 'active') {
      active.startupFailure = 'device-lost';
      return;
    }
    if (active.deviceChangeReported) return;
    active.suppressNextDeviceChange = true;
    this.#deviceChangeIncludedDefaultInvalidation = true;
    this.#notifyDefaultInvalidated(binding.generation);
  }

  #notifyDefaultInvalidated(bindingGeneration: number): void {
    try {
      this.#callbacks.onDefaultInvalidated(bindingGeneration);
    } catch {
      // Capture remains authoritative if an ancillary invalidation observer fails.
    }
  }

  #handleCaptureFailure(generation: number, reason: CaptureStopReason): void {
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
    const failureGeneration = ++this.#generation;
    void this.#teardownActive(false).finally(() => {
      if (this.#generation === failureGeneration) this.#callbacks.onUnexpectedStop(reason);
    });
  }

  readonly #onDeviceChange = () => {
    if (this.#disposed) return;
    const active = this.#active;
    if (
      active === null &&
      this.#startingDefaultGeneration !== null &&
      this.#startingDefaultGeneration === this.#generation
    ) {
      this.#defaultChangedDuringStartup = true;
      this.#deviceChangeIncludedDefaultInvalidation = true;
    }
    if (active?.followsSystemDefault === true) {
      this.#deviceChangeIncludedDefaultInvalidation = true;
      if (active.suppressNextDeviceChange) {
        active.suppressNextDeviceChange = false;
        active.deviceChangeReported = true;
      } else if (active.phase === 'active') {
        active.deviceChangeReported = true;
        this.#notifyDefaultInvalidated(active.microphone.generation);
      } else {
        active.defaultChangePending = true;
      }
    }
    if (this.#deviceTimer !== null) this.#environment.clearTimeout(this.#deviceTimer);
    this.#deviceTimer = this.#environment.setTimeout(() => {
      this.#deviceTimer = null;
      const defaultInvalidated = this.#deviceChangeIncludedDefaultInvalidation;
      this.#deviceChangeIncludedDefaultInvalidation = false;
      try {
        this.#callbacks.onDevicesChanged(defaultInvalidated);
      } catch {
        // Device enumeration is coordinated by main and can be retried by the next invalidation.
      }
    }, DEVICE_CHANGE_DEBOUNCE_MS);
  };
}

export function createBrowserCaptureEnvironment(workletModuleUrl: string): CaptureEnvironment {
  return {
    mediaDevices: navigator.mediaDevices,
    createAudioContext: () => new AudioContext({ latencyHint: 'interactive' }),
    createWorkletNode: (context) =>
      new AudioWorkletNode(context, CAPTURE_WORKLET_PROCESSOR_NAME, {
        numberOfInputs: 2,
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

function liveAudioTrack(stream: MediaStream): MediaStreamTrack | null {
  return stream.getAudioTracks().find((track) => track.readyState !== 'ended') ?? null;
}
