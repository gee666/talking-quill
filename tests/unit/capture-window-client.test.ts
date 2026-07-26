import { EventEmitter } from 'node:events';
import { randomUUID } from 'node:crypto';
import { describe, expect, it, vi, type Mock } from 'vitest';
import {
  CaptureClientError,
  CaptureWindowClient,
} from '../../app/src/main/audio/capture-window-client';
import type { CaptureMessageChannelFactory } from '../../app/src/main/audio/capture-window-client';
import type { CapturePortCommand } from '../../app/src/shared/ipc/capture-port';

class FakeMainPort extends EventEmitter {
  readonly start = vi.fn();
  readonly close: Mock<() => void>;
  readonly postMessage = vi.fn<(message: unknown) => void>();

  constructor(autoEmitClose = true) {
    super();
    this.close = vi.fn(() => {
      if (autoEmitClose) this.emit('close');
    });
  }

  receive(data: unknown): void {
    this.emit('message', { data });
  }
}

function harness() {
  const port1 = new FakeMainPort();
  const port2 = new FakeMainPort();
  const webContents = {
    postMessage: vi.fn(),
  };
  const channelFactory: CaptureMessageChannelFactory = () => ({ port1, port2 });
  const client = new CaptureWindowClient(channelFactory);
  client.attach(webContents as unknown as Electron.WebContents);
  return { client, port1, port2, webContents };
}

function lastCommand(port: FakeMainPort): CapturePortCommand {
  return port.postMessage.mock.lastCall?.[0] as CapturePortCommand;
}

describe('CaptureWindowClient', () => {
  it('transfers a versioned port and correlates start responses', async () => {
    const test = harness();
    expect(test.port1.start).toHaveBeenCalledOnce();
    expect(test.webContents.postMessage).toHaveBeenCalledWith(
      'capture:port',
      { protocolVersion: 1 },
      [test.port2],
    );

    const captureId = randomUUID();
    const starting = test.client.start(null, captureId);
    const command = lastCommand(test.port1);
    expect(command).toMatchObject({ type: 'stream:start', captureId });
    test.port1.receive({
      type: 'stream:started',
      requestId: command.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
    });
    await expect(starting).resolves.toMatchObject({
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
    });
    const activating = test.client.activate(captureId);
    const activateCommand = lastCommand(test.port1);
    expect(activateCommand.type).toBe('stream:activate');
    test.port1.receive({
      type: 'stream:activated',
      requestId: activateCommand.requestId,
      captureId,
    });
    await expect(activating).resolves.toBeUndefined();
  });

  it('fails closed on an active frame gap instead of permanently stalling', async () => {
    const test = harness();
    const captureId = randomUUID();
    const frame = vi.fn();
    const stopped = vi.fn();
    test.client.onFrame(frame);
    test.client.onUnexpectedStop(stopped);
    const starting = test.client.start(null, captureId);
    const command = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:started',
      requestId: command.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });
    await starting;
    test.port1.receive({
      type: 'stream:frame',
      captureId,
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.1,
    });
    expect(frame).toHaveBeenCalledOnce();
    test.port1.receive({
      type: 'stream:frame',
      captureId,
      sequence: 2,
      samples: new Float32Array(320),
      rms: 0.1,
    });
    expect(test.port1.close).toHaveBeenCalledOnce();
    expect(stopped).toHaveBeenCalledWith(captureId, 'capture-unavailable');
  });

  it('fails closed on an invalid active PCM sample', async () => {
    const test = harness();
    const captureId = randomUUID();
    const stopped = vi.fn();
    test.client.onUnexpectedStop(stopped);
    const starting = test.client.start(null, captureId);
    const command = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:started',
      requestId: command.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });
    await starting;
    test.port1.receive({
      type: 'stream:frame',
      captureId,
      sequence: 0,
      samples: Float32Array.from([1.01]),
      rms: 0.1,
    });
    expect(test.port1.close).toHaveBeenCalledOnce();
    expect(stopped).toHaveBeenCalledWith(captureId, 'capture-unavailable');
  });

  it('rejects a start whose capture stopped before its acknowledgement arrived', async () => {
    const test = harness();
    const captureId = randomUUID();
    const stopped = vi.fn();
    test.client.onUnexpectedStop(stopped);
    const starting = test.client.start(null, captureId);
    const command = lastCommand(test.port1);

    test.port1.receive({
      type: 'stream:stopped',
      requestId: null,
      captureId,
      reason: 'error',
    });
    test.port1.receive({
      type: 'stream:started',
      requestId: command.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });

    await expect(starting).rejects.toMatchObject({ code: 'capture-unavailable' });
    expect(stopped).toHaveBeenCalledWith(captureId, 'capture-unavailable');
  });

  it('maps device loss distinctly from renderer and transport failures', async () => {
    const test = harness();
    const captureId = randomUUID();
    const stopped = vi.fn();
    test.client.onUnexpectedStop(stopped);
    const starting = test.client.start(null, captureId);
    const command = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:started',
      requestId: command.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });
    await starting;

    test.port1.receive({
      type: 'stream:stopped',
      requestId: null,
      captureId,
      reason: 'device-lost',
    });
    expect(stopped).toHaveBeenCalledWith(captureId, 'device-unavailable');
  });

  it('forwards a final partial PCM frame before acknowledging stop', async () => {
    const test = harness();
    const captureId = randomUUID();
    const frame = vi.fn();
    test.client.onFrame(frame);
    const starting = test.client.start(null, captureId);
    const startCommand = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:started',
      requestId: startCommand.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });
    await starting;

    const stopping = test.client.stop(captureId);
    const stopCommand = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:frame',
      captureId,
      sequence: 0,
      samples: Float32Array.from([0.25, -0.25]),
      rms: 0.25,
    });
    test.port1.receive({
      type: 'stream:stopped',
      requestId: stopCommand.requestId,
      captureId,
      reason: 'requested',
    });
    await stopping;
    expect(frame).toHaveBeenCalledWith(
      expect.objectContaining({ captureId, samples: Float32Array.from([0.25, -0.25]) }),
    );
  });

  it('ignores an asynchronous close from a superseded port generation', async () => {
    const oldPort = new FakeMainPort(false);
    const newPort = new FakeMainPort();
    const oldTransferredPort = new FakeMainPort();
    const newTransferredPort = new FakeMainPort();
    const channels = [
      { port1: oldPort, port2: oldTransferredPort },
      { port1: newPort, port2: newTransferredPort },
    ];
    const client = new CaptureWindowClient(() => {
      const channel = channels.shift();
      if (channel === undefined) throw new Error('No test channel');
      return channel;
    });
    const contents = { postMessage: vi.fn() } as unknown as Electron.WebContents;
    client.attach(contents);
    client.attach(contents);
    oldPort.emit('close');

    const pending = client.listDevices();
    const command = lastCommand(newPort);
    newPort.receive({ type: 'devices:list-result', requestId: command.requestId, devices: [] });
    await expect(pending).resolves.toEqual([]);
  });

  it('rejects pending requests and notifies every observer when the capture port closes', async () => {
    const test = harness();
    const captureId = randomUUID();
    const starting = test.client.start(null, captureId);
    const startCommand = lastCommand(test.port1);
    test.port1.receive({
      type: 'stream:started',
      requestId: startCommand.requestId,
      captureId,
      sampleRate: 16_000,
      channelCount: 1,
      activeMicrophoneId: null,
      preferredUnavailable: false,
    });
    await starting;
    const laterObserver = vi.fn();
    test.client.onUnexpectedStop(() => {
      throw new Error('observer failed');
    });
    test.client.onUnexpectedStop(laterObserver);
    const pending = test.client.listDevices();

    test.port1.emit('close');

    await expect(pending).rejects.toBeInstanceOf(CaptureClientError);
    expect(laterObserver).toHaveBeenCalledWith(captureId, 'capture-unavailable');
  });
});
