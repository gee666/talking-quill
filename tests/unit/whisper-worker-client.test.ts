import { EventEmitter } from 'node:events';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  WhisperWorkerClient,
  type ModelUseAcquirer,
  type WhisperWorkerSpawner,
} from '../../app/src/main/transcription/whisper-worker-client';
import { WhisperWorkerRequestSchema } from '../../app/src/shared/schemas/whisper-protocol';

class FakeWorker extends EventEmitter {
  readonly pid = undefined;
  killed = false;
  readonly messages: unknown[] = [];
  autoExitOnKill = true;
  shutdownOperation: 'shutdown' | 'memory-pressure' = 'shutdown';

  postMessage(message: unknown): void {
    this.messages.push(message);
    const request = WhisperWorkerRequestSchema.parse(message);
    if (request.type === 'shutdown') {
      queueMicrotask(() => {
        this.respond(request.requestId, {
          type: 'acknowledged',
          operation: this.shutdownOperation,
        });
        this.exit(0);
      });
    }
  }

  kill(): boolean {
    if (this.killed) return false;
    this.killed = true;
    if (this.autoExitOnKill) this.exit(1);
    return true;
  }

  exit(code = 1): void {
    this.emit('exit', code);
  }

  ready(): void {
    this.emit('message', {
      version: 1,
      requestId: 'worker-ready',
      ok: true,
      result: { type: 'ready', networkGuarded: true, networkProbeCompleted: false },
    });
  }

  invalidResponse(): void {
    this.emit('message', { invalid: true });
  }

  replyAcknowledged(
    operation: 'session-open' | 'session-push' | 'unload' | 'memory-pressure',
  ): void {
    this.replyToLatest({ type: 'acknowledged', operation }, operation);
  }

  replyAcknowledgementFor(
    requestType: string,
    operation: 'session-open' | 'session-push' | 'unload' | 'memory-pressure' | 'shutdown',
  ): void {
    this.replyToLatest({ type: 'acknowledged', operation }, requestType);
  }

  replyTranscription(text: string): void {
    this.replyToLatest({
      type: 'transcription',
      value: {
        text,
        modelId: 'Xenova/whisper-small',
        durationMs: 1,
        pipeline: { loadCount: 1, reused: true, loadDurationMs: 1 },
      },
    });
  }

  private replyToLatest(result: unknown, type?: string): void {
    const requests = this.messages.map((message) => WhisperWorkerRequestSchema.parse(message));
    for (const request of [...requests].reverse()) {
      if (type !== undefined && request.type !== type) continue;
      this.respond(request.requestId, result);
      return;
    }
    throw new Error('No matching worker request');
  }

  private respond(requestId: string, result: unknown): void {
    this.emit('message', { version: 1, requestId, ok: true, result });
  }
}

const options = { modelId: 'Xenova/whisper-small' as const, sampleRate: 16_000 as const };
const readyAcquirer: ModelUseAcquirer = (modelId) =>
  Promise.resolve({
    status: {
      modelId,
      state: 'ready',
      downloadedBytes: 1,
      totalBytes: 1,
      detail: null,
      repairable: false,
    },
    release: () => undefined,
  });

afterEach(() => vi.useRealTimers());

describe('WhisperWorkerClient', () => {
  it('suppresses stale responses and recovers with a healthy replacement generation', async () => {
    const workers: FakeWorker[] = [];
    const spawn: WhisperWorkerSpawner = () => {
      const worker = new FakeWorker();
      workers.push(worker);
      queueMicrotask(() => worker.ready());
      return worker;
    };
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      spawn,
      acquireModelUse: readyAcquirer,
    });
    const first = client.transcribe(new Float32Array([0.1]), options, new AbortController().signal);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    workers[0]?.kill();
    await expect(first).rejects.toMatchObject({ code: 'WORKER_CRASHED' });

    const second = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(workers.length).toBeGreaterThanOrEqual(2));
    workers[0]?.replyTranscription('stale');
    await vi.waitFor(() => expect(latestRequestType(workers.at(-1))).toBe('transcribe'));
    workers.at(-1)?.replyTranscription('recovered');
    await expect(second).resolves.toMatchObject({ text: 'recovered' });
    await client.close();
  });

  it('waits for the actual exit on cancellation and then starts a fresh generation', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const controller = new AbortController();
    let settled = false;
    const request = client
      .transcribe(new Float32Array([0.1]), options, controller.signal)
      .finally(() => {
        settled = true;
      });
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    controller.abort();
    await vi.waitFor(() => expect(workers[0]?.killed).toBe(true));
    await Promise.resolve();
    expect(settled).toBe(false);
    workers[0]?.exit(1);
    await expect(request).rejects.toMatchObject({ code: 'CANCELLED' });

    const next = client.transcribe(new Float32Array([0.2]), options, new AbortController().signal);
    await vi.waitFor(() => expect(workers).toHaveLength(2));
    workers[1]?.ready();
    await vi.waitFor(() => expect(latestRequestType(workers[1])).toBe('transcribe'));
    workers[1]?.replyTranscription('healthy');
    await expect(next).resolves.toMatchObject({ text: 'healthy' });
    const replacementWorker = workers[1];
    if (replacementWorker !== undefined) replacementWorker.autoExitOnKill = true;
    await client.close();
  });

  it('preempts an active streaming push and resolves cancel only after exit', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    let cancelled = false;
    const cancellation = session.cancel().then(() => {
      cancelled = true;
    });
    await Promise.resolve();
    expect(cancelled).toBe(false);
    workers[0]?.exit(1);
    await cancellation;
    await expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    await expect(session.finish()).rejects.toMatchObject({ code: 'CANCELLED' });
    await client.close();
  });

  it('gates worker startup on safe manager readiness and distinguishes corruption', async () => {
    let spawned = false;
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      spawn: () => {
        spawned = true;
        return new FakeWorker();
      },
      acquireModelUse: (modelId) =>
        Promise.resolve({
          status: {
            modelId,
            state: 'corrupt',
            downloadedBytes: 0,
            totalBytes: 1,
            detail: 'checksum',
            repairable: true,
          },
          release: () => undefined,
        }),
    });
    await expect(
      client.transcribe(new Float32Array([0.1]), options, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'MODEL_CORRUPT' });
    expect(spawned).toBe(false);
  });

  it('rejects mismatched acknowledgements for open, push, pressure, and shutdown', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);

    const badOpen = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledgementFor('session-open', 'memory-pressure');
    await expect(badOpen).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const opening = client.startSession(options);
    await vi.waitFor(() =>
      expect(
        workers[0]?.messages.filter(
          (message) => WhisperWorkerRequestSchema.parse(message).type === 'session-open',
        ),
      ).toHaveLength(2),
    );
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    workers[0]?.replyAcknowledgementFor('session-push', 'unload');
    await expect(push).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const pressure = client.memoryPressure();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('memory-pressure'));
    workers[0]?.replyAcknowledgementFor('memory-pressure', 'unload');
    await expect(pressure).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const worker = workers[0];
    if (worker === undefined) throw new Error('Worker missing');
    worker.shutdownOperation = 'memory-pressure';
    await expect(client.close()).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });
  });

  it('uses non-cancel errors for protocol-directed termination', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const request = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    workers[0]?.invalidResponse();
    await Promise.resolve();
    let settled = false;
    void request.catch(() => {
      settled = true;
    });
    expect(settled).toBe(false);
    workers[0]?.exit(1);
    await expect(request).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });
    await client.close();
  });

  it('bounds automatic exponential restarts and resets backoff after stability', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    let crashImmediately = true;
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: readyAcquirer,
      spawn: () => {
        const worker = new FakeWorker();
        workers.push(worker);
        queueMicrotask(() => {
          if (crashImmediately) worker.kill();
          else worker.ready();
        });
        return worker;
      },
    });
    const request = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    vi.runAllTicks();
    await expect(request).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    await vi.advanceTimersByTimeAsync(20_000);
    expect(workers).toHaveLength(6);

    crashImmediately = false;
    const recovery = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 10; flush += 1) {
      await Promise.resolve();
      vi.runAllTicks();
    }
    expect(workers).toHaveLength(7);
    await vi.waitFor(() => expect(latestRequestType(workers.at(-1))).toBe('transcribe'));
    workers.at(-1)?.replyTranscription('stable');
    await expect(recovery).resolves.toMatchObject({ text: 'stable' });
    await vi.advanceTimersByTimeAsync(30_000);
    workers.at(-1)?.kill();
    await vi.advanceTimersByTimeAsync(99);
    expect(workers).toHaveLength(7);
    await vi.advanceTimersByTimeAsync(1);
    expect(workers).toHaveLength(8);
    await client.close();
  });
});

function createClient(workers: FakeWorker[], autoExitOnKill = true): WhisperWorkerClient {
  return new WhisperWorkerClient({
    cacheDirectory: 'models',
    workerPath: 'worker.js',
    acquireModelUse: readyAcquirer,
    spawn: () => {
      const worker = new FakeWorker();
      worker.autoExitOnKill = autoExitOnKill;
      workers.push(worker);
      queueMicrotask(() => worker.ready());
      return worker;
    },
  });
}

function latestRequestType(worker: FakeWorker | undefined): string | undefined {
  if (worker === undefined) return undefined;
  const message = worker.messages.at(-1);
  return message === undefined ? undefined : WhisperWorkerRequestSchema.parse(message).type;
}
