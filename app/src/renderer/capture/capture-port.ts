import {
  CAPTURE_PORT_PROTOCOL_VERSION,
  CAPTURE_PORT_WINDOW_MESSAGE,
} from '../../shared/constants/audio';
import {
  CapturePortCommandSchema,
  CapturePortDescriptorSchema,
  CapturePortMessageSchema,
  type CapturePortMessage,
} from '../../shared/ipc/capture-port';
import type { MicrophoneDevice } from '../../shared/schemas/audio';
import { CaptureEngineError } from './capture-engine';
import type { CaptureEngine, CaptureStopReason } from './capture-engine';

interface PendingStart {
  readonly requestId: string;
  readonly captureId: string;
}

type CapturePhase = 'idle' | 'starting' | 'prepared' | 'activating' | 'active' | 'stopping';

export class CapturePortController {
  readonly #port: MessagePort;
  readonly #engine: CaptureEngine;
  #captureId: string | null = null;
  #phase: CapturePhase = 'idle';
  #pendingStart: PendingStart | null = null;
  #stopPromise: Promise<void> | null = null;
  #sequence = 0;
  #closed = false;

  constructor(port: MessagePort, engine: CaptureEngine) {
    this.#port = port;
    this.#engine = engine;
    this.#port.addEventListener('message', this.#onMessage);
    this.#port.addEventListener('messageerror', this.#onMessageError);
    this.#port.addEventListener('close', this.#onPortClose);
    this.#port.start();
    this.#send({ type: 'port:ready', protocolVersion: CAPTURE_PORT_PROTOCOL_VERSION });
  }

  close(): void {
    if (this.#closed) return;
    this.#closed = true;
    this.#port.removeEventListener('message', this.#onMessage);
    this.#port.removeEventListener('messageerror', this.#onMessageError);
    this.#port.removeEventListener('close', this.#onPortClose);
    this.#captureId = null;
    this.#phase = 'idle';
    this.#pendingStart = null;
    this.#stopPromise = null;
    this.#port.close();
    this.#engine.disposeImmediately();
  }

  notifyDevicesChanged(devices: readonly MicrophoneDevice[]): void {
    this.#send({ type: 'devices:changed', devices: [...devices] });
  }

  notifyFrame(samples: Float32Array, rms: number): void {
    const captureId = this.#captureId;
    if (captureId === null || this.#closed) return;
    const sequence = this.#sequence;
    this.#sequence += 1;
    // Electron's renderer-to-main port bridge does not reliably preserve a message when a
    // nested typed-array buffer is transferred. The worklet-owned frame is structured-cloned
    // directly; CaptureEngine already validated it and main validates the IPC boundary again.
    const frameSamples =
      samples.buffer instanceof ArrayBuffer
        ? (samples as Float32Array<ArrayBuffer>)
        : Float32Array.from(samples);
    this.#post({ type: 'stream:frame', captureId, sequence, samples: frameSamples, rms });
  }

  notifyUnexpectedStop(reason: CaptureStopReason): void {
    const captureId = this.#captureId;
    if (captureId === null) return;
    this.#captureId = null;
    this.#phase = 'idle';
    this.#pendingStart = null;
    this.#send({ type: 'stream:stopped', requestId: null, captureId, reason });
  }

  readonly #onMessage = (event: MessageEvent<unknown>) => {
    const parsed = CapturePortCommandSchema.safeParse(event.data);
    if (!parsed.success) return;
    const command = parsed.data;
    if (command.type === 'devices:list') {
      void this.#engine.listDevices().then(
        (devices) =>
          this.#send({
            type: 'devices:list-result',
            requestId: command.requestId,
            devices: [...devices],
          }),
        () => this.#sendError(command.requestId, null, 'capture-failed'),
      );
      return;
    }
    if (command.type === 'stream:start') {
      void this.#start(
        command.requestId,
        command.captureId,
        command.preferredMicrophoneId,
        command.includeSystemAudio,
      );
      return;
    }
    if (command.type === 'stream:activate') {
      void this.#activate(command.requestId, command.captureId);
      return;
    }
    void this.#stop(command.requestId, command.captureId);
  };

  readonly #onMessageError = () => this.close();
  readonly #onPortClose = () => this.close();

  async #start(
    requestId: string,
    captureId: string,
    preferredMicrophoneId: string | null,
    includeSystemAudio: boolean,
  ): Promise<void> {
    if (this.#phase !== 'idle') {
      this.#sendError(requestId, captureId, 'capture-failed');
      return;
    }
    const pending = { requestId, captureId };
    this.#captureId = captureId;
    this.#phase = 'starting';
    this.#pendingStart = pending;
    this.#sequence = 0;
    try {
      const result = await this.#engine.start(preferredMicrophoneId, includeSystemAudio);
      if (this.#pendingStart !== pending || !this.#isCaptureInPhase(captureId, 'starting')) return;
      this.#pendingStart = null;
      this.#phase = 'prepared';
      this.#send({ type: 'stream:started', requestId, captureId, ...result });
    } catch (error: unknown) {
      if (this.#pendingStart !== pending || !this.#isCaptureInPhase(captureId, 'starting')) return;
      this.#pendingStart = null;
      this.#captureId = null;
      this.#phase = 'idle';
      this.#sendError(
        requestId,
        captureId,
        error instanceof CaptureEngineError ? error.code : 'capture-failed',
      );
    }
  }

  async #activate(requestId: string, captureId: string): Promise<void> {
    if (this.#captureId !== captureId || this.#phase !== 'prepared') {
      this.#sendError(requestId, captureId, 'capture-failed');
      return;
    }
    this.#phase = 'activating';
    try {
      await this.#engine.activate();
      if (this.#closed) return;
      if (!this.#isCaptureInPhase(captureId, 'activating')) {
        this.#sendError(requestId, captureId, 'capture-failed');
        return;
      }
      this.#phase = 'active';
      this.#send({ type: 'stream:activated', requestId, captureId });
    } catch (error: unknown) {
      if (this.#closed) return;
      if (this.#isCaptureInPhase(captureId, 'activating')) {
        this.#captureId = null;
        this.#phase = 'idle';
      }
      this.#sendError(
        requestId,
        captureId,
        error instanceof CaptureEngineError ? error.code : 'capture-failed',
      );
    }
  }

  async #stop(requestId: string, captureId: string): Promise<void> {
    if (this.#captureId !== captureId || this.#phase === 'idle') {
      this.#send({ type: 'stream:stopped', requestId, captureId, reason: 'requested' });
      return;
    }
    if (this.#phase === 'stopping') {
      try {
        await this.#stopPromise;
        this.#send({ type: 'stream:stopped', requestId, captureId, reason: 'requested' });
      } catch {
        this.#sendError(requestId, captureId, 'capture-failed');
      }
      return;
    }

    const pendingStart = this.#pendingStart;
    this.#pendingStart = null;
    this.#phase = 'stopping';
    const stopping = this.#engine.stop();
    this.#stopPromise = stopping;
    try {
      // Keep routing frames until the worklet flush completed inside engine.stop().
      await stopping;
      if (this.#isCaptureInPhase(captureId, 'stopping')) {
        this.#captureId = null;
        this.#phase = 'idle';
      }
      if (pendingStart !== null) {
        this.#sendError(pendingStart.requestId, captureId, 'capture-failed');
      }
      this.#send({ type: 'stream:stopped', requestId, captureId, reason: 'requested' });
    } catch {
      this.#sendError(requestId, captureId, 'capture-failed');
      this.close();
    } finally {
      if (this.#stopPromise === stopping) this.#stopPromise = null;
    }
  }

  #isCaptureInPhase(captureId: string, phase: CapturePhase): boolean {
    return !this.#closed && this.#captureId === captureId && this.#phase === phase;
  }

  #sendError(
    requestId: string | null,
    captureId: string | null,
    code: CaptureEngineError['code'],
  ): void {
    this.#send({ type: 'request:error', requestId, captureId, code });
  }

  #send(message: CapturePortMessage): boolean {
    try {
      return this.#post(CapturePortMessageSchema.parse(message));
    } catch {
      this.close();
      return false;
    }
  }

  #post(message: CapturePortMessage): boolean {
    if (this.#closed) return false;
    try {
      this.#port.postMessage(message);
      return true;
    } catch {
      this.close();
      return false;
    }
  }
}

export function readCapturePortEvent(event: MessageEvent<unknown>): MessagePort | null {
  if (event.source !== window || event.data === null || typeof event.data !== 'object') return null;
  if (Reflect.get(event.data, 'type') !== CAPTURE_PORT_WINDOW_MESSAGE) return null;
  const descriptor = CapturePortDescriptorSchema.safeParse(Reflect.get(event.data, 'descriptor'));
  const port = event.ports[0];
  return descriptor.success && port !== undefined ? port : null;
}
