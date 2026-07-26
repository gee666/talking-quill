import { MessageChannelMain, type MessagePortMain, type WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import {
  CAPTURE_COMMAND_TIMEOUT_MS,
  CAPTURE_PORT_PROTOCOL_VERSION,
} from '../../shared/constants/audio';
import {
  CapturePortDescriptorSchema,
  CapturePortMessageSchema,
  type CapturePortCommand,
  type CapturePortMessage,
} from '../../shared/ipc/capture-port';
import type { MicrophoneDevice } from '../../shared/schemas/audio';
import { transferPort } from '../ipc/port-transfer';

export interface CaptureStarted {
  readonly captureId: string;
  readonly activeMicrophoneId: string | null;
  readonly preferredUnavailable: boolean;
  readonly sampleRate: 16_000;
  readonly channelCount: 1;
}

export interface CaptureFrame {
  readonly captureId: string;
  readonly sequence: number;
  readonly samples: Float32Array;
  readonly rms: number;
}

export type UnexpectedCaptureStopReason = 'device-unavailable' | 'capture-unavailable';

export class CaptureClientError extends Error {
  readonly code:
    | 'permission-denied'
    | 'no-device'
    | 'device-unavailable'
    | 'unsupported-audio-format'
    | 'worklet-unavailable'
    | 'capture-failed'
    | 'capture-unavailable';

  constructor(code: CaptureClientError['code']) {
    super(code);
    this.name = 'CaptureClientError';
    this.code = code;
  }
}

interface PendingRequest {
  readonly resolve: (message: CapturePortMessage) => void;
  readonly reject: (error: CaptureClientError) => void;
  readonly timer: ReturnType<typeof setTimeout>;
}

export interface CaptureMessageChannel {
  readonly port1: MessagePortMain;
  readonly port2: MessagePortMain;
}

export type CaptureMessageChannelFactory = () => CaptureMessageChannel;

export class CaptureWindowClient {
  readonly #channelFactory: CaptureMessageChannelFactory;
  readonly #pending = new Map<string, PendingRequest>();
  readonly #frameListeners = new Set<(frame: CaptureFrame) => void>();
  readonly #deviceListeners = new Set<(devices: readonly MicrophoneDevice[]) => void>();
  readonly #stopListeners = new Set<
    (captureId: string, reason: UnexpectedCaptureStopReason) => void
  >();
  #port: MessagePortMain | null = null;
  #activeCaptureId: string | null = null;
  #lastSequence = -1;

  constructor(channelFactory: CaptureMessageChannelFactory = () => new MessageChannelMain()) {
    this.#channelFactory = channelFactory;
  }

  attach(webContents: WebContents): void {
    this.#closePort();
    const channel = this.#channelFactory();
    const port = channel.port1;
    this.#port = port;
    port.on('message', (event) => {
      if (this.#port === port) this.#onMessage(event.data);
    });
    port.on('close', () => this.#handlePortClosed(port));
    port.start();
    const descriptor = CapturePortDescriptorSchema.parse({
      protocolVersion: CAPTURE_PORT_PROTOCOL_VERSION,
    });
    try {
      transferPort(webContents, 'capture:port', descriptor, channel.port2);
    } catch (error: unknown) {
      this.#closePort();
      channel.port2.close();
      throw error;
    }
  }

  async listDevices(): Promise<readonly MicrophoneDevice[]> {
    const response = await this.#request({ type: 'devices:list', requestId: randomUUID() });
    if (response.type !== 'devices:list-result') throw new CaptureClientError('capture-failed');
    return response.devices;
  }

  async start(
    preferredMicrophoneId: string | null,
    captureId: string = randomUUID(),
  ): Promise<CaptureStarted> {
    if (this.#activeCaptureId !== null) await this.stop(this.#activeCaptureId);
    this.#activeCaptureId = captureId;
    this.#lastSequence = -1;
    const response = await this.#request({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId,
      preferredMicrophoneId,
    }).catch((error: unknown) => {
      if (this.#activeCaptureId === captureId) this.#activeCaptureId = null;
      throw error;
    });
    if (response.type !== 'stream:started' || response.captureId !== captureId) {
      if (this.#activeCaptureId === captureId) this.#activeCaptureId = null;
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId !== captureId) {
      throw new CaptureClientError('capture-unavailable');
    }
    return {
      captureId,
      activeMicrophoneId: response.activeMicrophoneId,
      preferredUnavailable: response.preferredUnavailable,
      sampleRate: response.sampleRate,
      channelCount: response.channelCount,
    };
  }

  async activate(captureId: string): Promise<void> {
    if (this.#activeCaptureId !== captureId) throw new CaptureClientError('capture-failed');
    const response = await this.#request({
      type: 'stream:activate',
      requestId: randomUUID(),
      captureId,
    });
    if (response.type !== 'stream:activated' || response.captureId !== captureId) {
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId !== captureId) {
      throw new CaptureClientError('capture-unavailable');
    }
  }

  async stop(captureId: string = this.#activeCaptureId ?? ''): Promise<void> {
    if (captureId.length === 0) return;
    const response = await this.#request({
      type: 'stream:stop',
      requestId: randomUUID(),
      captureId,
    });
    if (response.type !== 'stream:stopped' || response.captureId !== captureId) {
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId === captureId) this.#activeCaptureId = null;
  }

  onFrame(listener: (frame: CaptureFrame) => void): () => void {
    this.#frameListeners.add(listener);
    return () => this.#frameListeners.delete(listener);
  }

  onDevicesChanged(listener: (devices: readonly MicrophoneDevice[]) => void): () => void {
    this.#deviceListeners.add(listener);
    return () => this.#deviceListeners.delete(listener);
  }

  onUnexpectedStop(
    listener: (captureId: string, reason: UnexpectedCaptureStopReason) => void,
  ): () => void {
    this.#stopListeners.add(listener);
    return () => this.#stopListeners.delete(listener);
  }

  reset(): void {
    this.#activeCaptureId = null;
    this.#closePort();
  }

  dispose(): void {
    this.reset();
    this.#frameListeners.clear();
    this.#deviceListeners.clear();
    this.#stopListeners.clear();
  }

  #request(command: CapturePortCommand): Promise<CapturePortMessage> {
    const port = this.#port;
    if (port === null) return Promise.reject(new CaptureClientError('capture-unavailable'));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.#pending.delete(command.requestId);
        reject(new CaptureClientError('capture-unavailable'));
      }, CAPTURE_COMMAND_TIMEOUT_MS);
      timer.unref();
      this.#pending.set(command.requestId, { resolve, reject, timer });
      try {
        port.postMessage(command);
      } catch {
        clearTimeout(timer);
        this.#pending.delete(command.requestId);
        reject(new CaptureClientError('capture-unavailable'));
      }
    });
  }

  #onMessage(input: unknown): void {
    const parsed = CapturePortMessageSchema.safeParse(input);
    if (!parsed.success) {
      if (this.#activeCaptureId !== null && readMessageType(input) === 'stream:frame') {
        this.#closePort();
      }
      return;
    }
    const message = parsed.data;
    if (message.type === 'port:ready') return;
    if (message.type === 'devices:changed') {
      for (const listener of this.#deviceListeners) {
        try {
          listener(message.devices);
        } catch {
          // One consumer must not block independent capture consumers.
        }
      }
      return;
    }
    if (message.type === 'stream:frame') {
      if (
        message.captureId !== this.#activeCaptureId ||
        message.sequence !== this.#lastSequence + 1
      ) {
        this.#closePort();
        return;
      }
      this.#lastSequence = message.sequence;
      for (const listener of this.#frameListeners) {
        try {
          listener(message);
        } catch {
          // A failed downstream observer must not break capture transport processing.
        }
      }
      return;
    }
    if (message.type === 'stream:stopped' && message.requestId === null) {
      if (this.#activeCaptureId === message.captureId) this.#activeCaptureId = null;
      this.#notifyUnexpectedStop(
        message.captureId,
        message.reason === 'device-lost' ? 'device-unavailable' : 'capture-unavailable',
      );
      return;
    }

    const requestId = message.requestId;
    if (requestId === null) return;
    const pending = this.#pending.get(requestId);
    if (pending === undefined) return;
    this.#pending.delete(requestId);
    clearTimeout(pending.timer);
    if (message.type === 'request:error') {
      pending.reject(new CaptureClientError(message.code));
    } else {
      pending.resolve(message);
    }
  }

  #handlePortClosed(port: MessagePortMain): void {
    if (this.#port !== port) return;
    this.#port = null;
    const activeCaptureId = this.#activeCaptureId;
    this.#activeCaptureId = null;
    for (const pending of this.#pending.values()) {
      clearTimeout(pending.timer);
      pending.reject(new CaptureClientError('capture-unavailable'));
    }
    this.#pending.clear();
    if (activeCaptureId !== null) {
      this.#notifyUnexpectedStop(activeCaptureId, 'capture-unavailable');
    }
  }

  #notifyUnexpectedStop(captureId: string, reason: UnexpectedCaptureStopReason): void {
    for (const listener of this.#stopListeners) {
      try {
        listener(captureId, reason);
      } catch {
        // Capture transport cleanup remains authoritative if an observer fails.
      }
    }
  }

  #closePort(): void {
    const port = this.#port;
    if (port === null) return;
    this.#handlePortClosed(port);
    port.close();
  }
}

function readMessageType(input: unknown): unknown {
  return typeof input === 'object' && input !== null ? Reflect.get(input, 'type') : null;
}
