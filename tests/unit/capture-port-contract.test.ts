import { randomUUID } from 'node:crypto';
import { describe, expect, it, vi } from 'vitest';
import { CapturePortController } from '../../app/src/renderer/capture/capture-port';
import type { CaptureEngine } from '../../app/src/renderer/capture/capture-engine';
import {
  CapturePortCommandSchema,
  CapturePortDescriptorSchema,
  CapturePortMessageSchema,
  type CapturePortCommand,
  type CapturePortMessage,
} from '../../app/src/shared/ipc/capture-port';

class FakeCapturePort extends EventTarget {
  readonly postMessage = vi.fn();
  readonly start = vi.fn();
  readonly close = vi.fn();

  emit(data: unknown): void {
    this.dispatchEvent(new MessageEvent('message', { data }));
  }
}

function strictFieldMutations(fixture: Record<string, unknown>): readonly unknown[] {
  const mutations: unknown[] = [];
  for (const key of Object.keys(fixture)) {
    if (key === 'type') continue;
    const missing = structuredClone(fixture);
    Reflect.deleteProperty(missing, key);
    mutations.push(
      missing,
      { ...structuredClone(fixture), [key]: null },
      { ...structuredClone(fixture), [key]: { wrong: true } },
      { ...structuredClone(fixture), [key]: 'x'.repeat(8_192) },
      { ...structuredClone(fixture), [key]: Number.MAX_SAFE_INTEGER + 1 },
    );
  }
  return mutations;
}

function deferred<Value>() {
  let resolvePromise!: (value: Value) => void;
  const promise = new Promise<Value>((resolve) => {
    resolvePromise = resolve;
  });
  return { promise, resolve: resolvePromise };
}

describe('capture MessagePort contracts', () => {
  it('fuzzes every command and message variant with strict deterministic mutations', () => {
    const requestId = randomUUID();
    const captureId = randomUUID();
    const commandsByType: {
      [Type in CapturePortCommand['type']]: Extract<CapturePortCommand, { type: Type }>;
    } = {
      'devices:list': { type: 'devices:list', requestId },
      'stream:start': {
        type: 'stream:start',
        requestId,
        captureId,
        preferredMicrophoneId: null,
        includeSystemAudio: false,
      },
      'stream:activate': { type: 'stream:activate', requestId, captureId },
      'stream:stop': { type: 'stream:stop', requestId, captureId },
    };
    const messagesByType: {
      [Type in CapturePortMessage['type']]: Extract<CapturePortMessage, { type: Type }>;
    } = {
      'port:ready': { type: 'port:ready', protocolVersion: 2 },
      'devices:list-result': {
        type: 'devices:list-result',
        requestId,
        devices: [{ deviceId: 'default', label: 'Default microphone', isDefault: true }],
      },
      'devices:changed': {
        type: 'devices:changed',
        devices: [{ deviceId: 'default', label: 'Default microphone', isDefault: true }],
      },
      'stream:started': {
        type: 'stream:started',
        requestId,
        captureId,
        sampleRate: 16_000,
        channelCount: 1,
        activeMicrophoneId: null,
        preferredUnavailable: false,
        systemAudioIncluded: false,
      },
      'stream:activated': { type: 'stream:activated', requestId, captureId },
      'stream:frame': {
        type: 'stream:frame',
        captureId,
        sequence: 0,
        samples: new Float32Array(320),
        rms: 0.1,
      },
      'stream:stopped': {
        type: 'stream:stopped',
        requestId,
        captureId,
        reason: 'requested',
      },
      'request:error': {
        type: 'request:error',
        requestId,
        captureId,
        code: 'capture-failed',
      },
    };
    const commands = Object.values(commandsByType);
    const messages = Object.values(messagesByType);
    expect(commands.map((command) => command.type)).toEqual([
      'devices:list',
      'stream:start',
      'stream:activate',
      'stream:stop',
    ]);
    expect(messages.map((message) => message.type)).toEqual([
      'port:ready',
      'devices:list-result',
      'devices:changed',
      'stream:started',
      'stream:activated',
      'stream:frame',
      'stream:stopped',
      'request:error',
    ]);
    for (const [schema, fixtures] of [
      [CapturePortCommandSchema, commands],
      [CapturePortMessageSchema, messages],
    ] as const) {
      for (const fixture of fixtures) {
        expect(schema.safeParse(fixture).success, fixture.type).toBe(true);
        const malformed = [
          null,
          [],
          { ...fixture, type: 'unknown' },
          { ...fixture, unknown: true },
          { ...fixture, requestId: 'not-a-uuid' },
          ...strictFieldMutations(fixture),
        ].filter((value) => !schema.safeParse(value).success);
        expect(malformed.length, fixture.type).toBeGreaterThan(Object.keys(fixture).length);
        for (const value of malformed) {
          expect(schema.safeParse(value).success, fixture.type).toBe(false);
        }
        if ('devices' in fixture) {
          for (const devices of [
            [{ deviceId: '', label: 'x', isDefault: true }],
            [{ deviceId: 'x', label: '', isDefault: true }],
            [{ deviceId: 'x', label: 'x', isDefault: 'yes' }],
            [{ deviceId: 'x', label: 'x', isDefault: true, unknown: true }],
          ]) {
            expect(schema.safeParse({ ...fixture, devices }).success, fixture.type).toBe(false);
          }
        }
      }
    }
  });

  it('accepts only the versioned descriptor and strict commands', () => {
    expect(CapturePortDescriptorSchema.parse({ protocolVersion: 2 })).toEqual({
      protocolVersion: 2,
    });
    expect(CapturePortDescriptorSchema.safeParse({ protocolVersion: 1 }).success).toBe(false);
    expect(
      CapturePortCommandSchema.safeParse({
        type: 'stream:start',
        requestId: randomUUID(),
        captureId: randomUUID(),
        preferredMicrophoneId: null,
        unexpected: true,
      }).success,
    ).toBe(false);
  });

  it('validates typed PCM frames, bounds, finite values, RMS, and sequence', () => {
    const base = {
      type: 'stream:frame',
      captureId: randomUUID(),
      sequence: 0,
      samples: new Float32Array(320),
      rms: 0.25,
    } as const;
    expect(CapturePortMessageSchema.safeParse(base).success).toBe(true);
    expect(
      CapturePortMessageSchema.safeParse({ ...base, samples: new Float32Array(321) }).success,
    ).toBe(false);
    expect(
      CapturePortMessageSchema.safeParse({ ...base, samples: Float32Array.from([Number.NaN]) })
        .success,
    ).toBe(false);
    expect(
      CapturePortMessageSchema.safeParse({ ...base, samples: Float32Array.from([1.01]) }).success,
    ).toBe(false);
    expect(
      CapturePortMessageSchema.safeParse({ ...base, samples: Float32Array.from([-1.01]) }).success,
    ).toBe(false);
    expect(CapturePortMessageSchema.safeParse({ ...base, samples: [0, 1] }).success).toBe(false);
    expect(CapturePortMessageSchema.safeParse({ ...base, sequence: -1 }).success).toBe(false);
    expect(CapturePortMessageSchema.safeParse({ ...base, rms: Number.NaN }).success).toBe(false);
    expect(CapturePortMessageSchema.safeParse({ ...base, rms: 1.1 }).success).toBe(false);
  });

  it('rejects raw errors and malformed device snapshots', () => {
    expect(
      CapturePortMessageSchema.safeParse({
        type: 'request:error',
        requestId: randomUUID(),
        captureId: null,
        code: 'raw-secret-stack',
      }).success,
    ).toBe(false);
    expect(
      CapturePortMessageSchema.safeParse({
        type: 'devices:changed',
        devices: [{ deviceId: '', label: '', isDefault: true }],
      }).success,
    ).toBe(false);
  });

  it('structured-clones typed PCM frames without a renderer-to-main transfer list', async () => {
    const port = new FakeCapturePort();
    const engine = {
      start: vi.fn(() =>
        Promise.resolve({
          activeMicrophoneId: 'default',
          preferredUnavailable: false,
          sampleRate: 16_000,
          channelCount: 1,
        }),
      ),
      stop: vi.fn(() => Promise.resolve()),
      activate: vi.fn(() => Promise.resolve()),
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately: vi.fn(),
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    const captureId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId,
      preferredMicrophoneId: null,
    });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:started', captureId }),
      ),
    );

    const samples = new Float32Array(320);
    controller.notifyFrame(samples, 0.25);

    expect(port.postMessage).toHaveBeenLastCalledWith(
      expect.objectContaining({ type: 'stream:frame', captureId, samples }),
    );
    expect(port.postMessage.mock.lastCall).toHaveLength(1);
    const posted = port.postMessage.mock.lastCall?.[0] as { readonly samples?: Float32Array };
    expect(posted.samples).toBe(samples);
    expect(samples.byteLength).toBe(1_280);
    controller.close();
  });

  it('forwards the engine flush frame before acknowledging stream stop', async () => {
    const port = new FakeCapturePort();
    const samples = Float32Array.from([0.25, -0.25]);
    const stop = vi.fn<() => Promise<void>>();
    const engine = {
      start: vi.fn(() =>
        Promise.resolve({
          activeMicrophoneId: 'default',
          preferredUnavailable: false,
          sampleRate: 16_000,
          channelCount: 1,
        }),
      ),
      stop,
      activate: vi.fn(() => Promise.resolve()),
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately: vi.fn(),
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    stop.mockImplementation(() => {
      controller.notifyFrame(samples, 0.25);
      return Promise.resolve();
    });
    const captureId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId,
      preferredMicrophoneId: null,
    });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:started', captureId }),
      ),
    );

    port.emit({ type: 'stream:stop', requestId: randomUUID(), captureId });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:stopped', captureId }),
      ),
    );
    const stopMessages = port.postMessage.mock.calls
      .map(([message]) => message as CapturePortMessage)
      .filter((message) => message.type === 'stream:frame' || message.type === 'stream:stopped');
    expect(stopMessages.map((message) => message.type)).toEqual(['stream:frame', 'stream:stopped']);
    expect(stopMessages[0]).toMatchObject({ samples });
    controller.close();
  });

  it('cancels a pending start out of band and rejects its pending request', async () => {
    const port = new FakeCapturePort();
    const pending = deferred<{
      activeMicrophoneId: string | null;
      preferredUnavailable: boolean;
      sampleRate: 16_000;
      channelCount: 1;
    }>();
    const start = vi.fn(() => pending.promise);
    const stop = vi.fn(() => Promise.resolve());
    const engine = {
      start,
      stop,
      activate: vi.fn(() => Promise.resolve()),
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately: vi.fn(),
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    const captureId = randomUUID();
    const startRequestId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: startRequestId,
      captureId,
      preferredMicrophoneId: null,
    });
    await vi.waitFor(() => expect(start).toHaveBeenCalledOnce());

    const stopRequestId = randomUUID();
    port.emit({ type: 'stream:stop', requestId: stopRequestId, captureId });

    await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(port.postMessage).toHaveBeenCalledWith({
      type: 'request:error',
      requestId: startRequestId,
      captureId,
      code: 'capture-failed',
    });
    expect(port.postMessage).toHaveBeenCalledWith({
      type: 'stream:stopped',
      requestId: stopRequestId,
      captureId,
      reason: 'requested',
    });
    pending.resolve({
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
      sampleRate: 16_000,
      channelCount: 1,
    });
    controller.close();
  });

  it('rejects duplicate activation without losing ownership of the active engine', async () => {
    const port = new FakeCapturePort();
    const activate = vi
      .fn<() => Promise<void>>()
      .mockResolvedValueOnce()
      .mockRejectedValueOnce(new Error('already active'));
    const stop = vi.fn(() => Promise.resolve());
    const engine = {
      start: vi.fn(() =>
        Promise.resolve({
          activeMicrophoneId: 'default',
          preferredUnavailable: false,
          sampleRate: 16_000,
          channelCount: 1,
        }),
      ),
      stop,
      activate,
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately: vi.fn(),
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    const captureId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId,
      preferredMicrophoneId: null,
    });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:started', captureId }),
      ),
    );

    port.emit({ type: 'stream:activate', requestId: randomUUID(), captureId });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:activated', captureId }),
      ),
    );
    const duplicateRequestId = randomUUID();
    port.emit({ type: 'stream:activate', requestId: duplicateRequestId, captureId });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith({
        type: 'request:error',
        requestId: duplicateRequestId,
        captureId,
        code: 'capture-failed',
      }),
    );

    port.emit({ type: 'stream:stop', requestId: randomUUID(), captureId });
    await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
    expect(activate).toHaveBeenCalledOnce();
    controller.close();
  });

  it('rejects activation during startup without orphaning the pending start', async () => {
    const port = new FakeCapturePort();
    const pending = deferred<{
      activeMicrophoneId: string | null;
      preferredUnavailable: boolean;
      sampleRate: 16_000;
      channelCount: 1;
    }>();
    const activate = vi.fn(() => Promise.reject(new Error('not prepared')));
    const stop = vi.fn(() => Promise.resolve());
    const engine = {
      start: vi.fn(() => pending.promise),
      stop,
      activate,
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately: vi.fn(),
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    const captureId = randomUUID();
    const startRequestId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: startRequestId,
      captureId,
      preferredMicrophoneId: null,
    });
    const activationRequestId = randomUUID();
    port.emit({ type: 'stream:activate', requestId: activationRequestId, captureId });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith({
        type: 'request:error',
        requestId: activationRequestId,
        captureId,
        code: 'capture-failed',
      }),
    );
    expect(activate).not.toHaveBeenCalled();

    pending.resolve({
      activeMicrophoneId: 'default',
      preferredUnavailable: false,
      sampleRate: 16_000,
      channelCount: 1,
    });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:started', requestId: startRequestId, captureId }),
      ),
    );
    port.emit({ type: 'stream:stop', requestId: randomUUID(), captureId });
    await vi.waitFor(() => expect(stop).toHaveBeenCalledOnce());
    controller.close();
  });

  it('closes the controller and engine when response validation fails', async () => {
    const port = new FakeCapturePort();
    const disposeImmediately = vi.fn();
    const engine = {
      start: vi.fn(() =>
        Promise.resolve({
          activeMicrophoneId: 'x'.repeat(1_025),
          preferredUnavailable: false,
          sampleRate: 16_000,
          channelCount: 1,
        }),
      ),
      stop: vi.fn(() => Promise.resolve()),
      activate: vi.fn(() => Promise.resolve()),
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately,
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    port.emit({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId: randomUUID(),
      preferredMicrophoneId: null,
    });

    await vi.waitFor(() => expect(disposeImmediately).toHaveBeenCalledOnce());
    expect(port.close).toHaveBeenCalledOnce();
    controller.close();
  });

  it('closes the controller and engine when a port send fails', async () => {
    const port = new FakeCapturePort();
    const disposeImmediately = vi.fn();
    const engine = {
      start: vi.fn(() =>
        Promise.resolve({
          activeMicrophoneId: 'default',
          preferredUnavailable: false,
          sampleRate: 16_000,
          channelCount: 1,
        }),
      ),
      stop: vi.fn(() => Promise.resolve()),
      activate: vi.fn(() => Promise.resolve()),
      listDevices: vi.fn(() => Promise.resolve([])),
      disposeImmediately,
    } as unknown as CaptureEngine;
    const controller = new CapturePortController(port as unknown as MessagePort, engine);
    const captureId = randomUUID();
    port.emit({
      type: 'stream:start',
      requestId: randomUUID(),
      captureId,
      preferredMicrophoneId: null,
    });
    await vi.waitFor(() =>
      expect(port.postMessage).toHaveBeenCalledWith(
        expect.objectContaining({ type: 'stream:started', captureId }),
      ),
    );
    port.postMessage.mockImplementationOnce(() => {
      throw new Error('port closed');
    });

    controller.notifyFrame(new Float32Array(320), 0.2);

    expect(port.close).toHaveBeenCalledOnce();
    expect(disposeImmediately).toHaveBeenCalledOnce();
  });
});
