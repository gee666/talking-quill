import { CaptureClientError } from './capture-client-error';
import { CaptureRequests } from './capture-requests';
export { CaptureClientError } from './capture-client-error';
import { MessageChannelMain, type MessagePortMain, type WebContents } from 'electron';
import { randomUUID } from 'node:crypto';
import { CAPTURE_PORT_PROTOCOL_VERSION } from '../../shared/constants/audio';
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
  readonly bindingGeneration: number;
  readonly systemAudioIncluded: boolean;
  readonly sampleRate: 16_000;
  readonly channelCount: 1;
}

export interface CaptureRebound {
  readonly captureId: string;
  readonly activeMicrophoneId: string | null;
  readonly bindingGeneration: number;
}

export interface CaptureFrame {
  readonly captureId: string;
  readonly sequence: number;
  readonly samples: Float32Array;
  readonly rms: number;
}

export type UnexpectedCaptureStopReason =
  'device-unavailable' | 'system-audio-unavailable' | 'capture-unavailable';

export interface CaptureMessageChannel {
  readonly port1: MessagePortMain;
  readonly port2: MessagePortMain;
}

export type CaptureMessageChannelFactory = () => CaptureMessageChannel;

export class CaptureWindowClient {
  readonly #channelFactory: CaptureMessageChannelFactory;
  readonly #roleForWebContents: (id: number) => string | null;
  readonly #requests = new CaptureRequests();
  readonly #frameListeners = new Set<(frame: CaptureFrame) => void>();
  readonly #deviceListeners = new Set<(defaultInvalidated: boolean) => void>();
  readonly #defaultInvalidationListeners = new Set<
    (captureId: string, bindingGeneration: number) => void
  >();
  readonly #stopListeners = new Set<
    (captureId: string, reason: UnexpectedCaptureStopReason) => void
  >();
  #port: MessagePortMain | null = null;
  #activeCaptureId: string | null = null;
  #lastSequence = -1;

  constructor(
    channelFactory: CaptureMessageChannelFactory = () => new MessageChannelMain(),
    roleForWebContents: (id: number) => string | null = () => null,
  ) {
    this.#channelFactory = channelFactory;
    this.#roleForWebContents = roleForWebContents;
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
      transferPort(
        webContents,
        this.#roleForWebContents(webContents.id),
        'capture',
        'capture:port',
        descriptor,
        channel.port2,
      );
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
    includeSystemAudio = false,
    signal?: AbortSignal,
  ): Promise<CaptureStarted> {
    if (this.#activeCaptureId !== null) await this.stop(this.#activeCaptureId, signal);
    this.#activeCaptureId = captureId;
    this.#lastSequence = -1;
    const response = await this.#request(
      {
        type: 'stream:start',
        requestId: randomUUID(),
        captureId,
        preferredMicrophoneId,
        includeSystemAudio,
      },
      signal,
    ).catch((error: unknown) => {
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
    if (response.systemAudioIncluded !== includeSystemAudio) {
      await this.stop(captureId).catch(() => this.#closePort());
      throw new CaptureClientError(
        includeSystemAudio ? 'system-audio-unavailable' : 'capture-failed',
      );
    }
    return {
      captureId,
      activeMicrophoneId: response.activeMicrophoneId,
      preferredUnavailable: response.preferredUnavailable,
      bindingGeneration: response.bindingGeneration,
      systemAudioIncluded: response.systemAudioIncluded,
      sampleRate: response.sampleRate,
      channelCount: response.channelCount,
    };
  }

  async activate(captureId: string, signal?: AbortSignal): Promise<void> {
    if (this.#activeCaptureId !== captureId) throw new CaptureClientError('capture-failed');
    const response = await this.#request(
      {
        type: 'stream:activate',
        requestId: randomUUID(),
        captureId,
      },
      signal,
    );
    if (response.type !== 'stream:activated' || response.captureId !== captureId) {
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId !== captureId) {
      throw new CaptureClientError('capture-unavailable');
    }
  }

  async rebindDefault(captureId: string, bindingGeneration: number): Promise<CaptureRebound> {
    if (this.#activeCaptureId !== captureId) throw new CaptureClientError('capture-unavailable');
    const response = await this.#request({
      type: 'stream:rebind-default',
      requestId: randomUUID(),
      captureId,
      bindingGeneration,
    });
    if (response.type !== 'stream:rebound' || response.captureId !== captureId) {
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId !== captureId) {
      throw new CaptureClientError('capture-unavailable');
    }
    return {
      captureId,
      activeMicrophoneId: response.activeMicrophoneId,
      bindingGeneration: response.bindingGeneration,
    };
  }

  async stop(captureId: string = this.#activeCaptureId ?? '', signal?: AbortSignal): Promise<void> {
    if (captureId.length === 0) return;
    const response = await this.#request(
      {
        type: 'stream:stop',
        requestId: randomUUID(),
        captureId,
      },
      signal,
    );
    if (response.type !== 'stream:stopped' || response.captureId !== captureId) {
      throw new CaptureClientError('capture-failed');
    }
    if (this.#activeCaptureId === captureId) this.#activeCaptureId = null;
  }

  onFrame(listener: (frame: CaptureFrame) => void): () => void {
    this.#frameListeners.add(listener);
    return () => this.#frameListeners.delete(listener);
  }

  onDevicesChanged(listener: (defaultInvalidated: boolean) => void): () => void {
    this.#deviceListeners.add(listener);
    return () => this.#deviceListeners.delete(listener);
  }

  onDefaultInvalidated(
    listener: (captureId: string, bindingGeneration: number) => void,
  ): () => void {
    this.#defaultInvalidationListeners.add(listener);
    return () => this.#defaultInvalidationListeners.delete(listener);
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
    this.#defaultInvalidationListeners.clear();
    this.#stopListeners.clear();
  }

  #request(command: CapturePortCommand, signal?: AbortSignal): Promise<CapturePortMessage> {
    return this.#requests.request(this.#port, () => this.#closePort(), command, signal);
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
    if (message.type === 'devices:invalidated') {
      for (const listener of this.#deviceListeners) {
        try {
          listener(message.defaultInvalidated);
        } catch {
          // One consumer must not block independent capture consumers.
        }
      }
      return;
    }
    if (message.type === 'stream:default-invalidated') {
      if (message.captureId !== this.#activeCaptureId) return;
      for (const listener of this.#defaultInvalidationListeners) {
        try {
          listener(message.captureId, message.bindingGeneration);
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
        message.reason === 'device-lost'
          ? 'device-unavailable'
          : message.reason === 'system-audio-lost'
            ? 'system-audio-unavailable'
            : 'capture-unavailable',
      );
      return;
    }

    this.#requests.settle(message);
  }

  #handlePortClosed(port: MessagePortMain): void {
    if (this.#port !== port) return;
    this.#port = null;
    const activeCaptureId = this.#activeCaptureId;
    this.#activeCaptureId = null;
    this.#requests.rejectAll();
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
