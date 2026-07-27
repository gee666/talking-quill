import { EventEmitter } from 'node:events';
import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  WhisperWorkerClient,
  type ModelUseAcquirer,
  type WhisperWorkerSpawner,
} from '../../app/src/main/transcription/whisper-worker-client';
import { WHISPER_SAMPLE_RATE } from '../../app/src/shared/constants/whisper';
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
      version: 2,
      requestId: 'worker-ready',
      ok: true,
      result: { type: 'ready', networkGuarded: true, networkProbeCompleted: false },
    });
  }

  invalidResponse(): void {
    this.emit('message', { invalid: true });
  }

  replyAcknowledged(
    operation: 'session-open' | 'session-push' | 'session-cancel' | 'unload' | 'memory-pressure',
  ): void {
    this.replyToLatest({ type: 'acknowledged', operation }, operation);
  }

  replyAcknowledgementFor(
    requestType: string,
    operation:
      | 'session-open'
      | 'session-push'
      | 'session-cancel'
      | 'unload'
      | 'memory-pressure'
      | 'shutdown',
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

  replyFailure(requestType: string, code: 'INFERENCE_FAILED', message: string): void {
    const requests = this.messages.map((request) => WhisperWorkerRequestSchema.parse(request));
    const request = [...requests].reverse().find((candidate) => candidate.type === requestType);
    if (request === undefined) throw new Error('No matching worker request');
    this.emit('message', {
      version: 2,
      requestId: request.requestId,
      ok: false,
      error: { code, message },
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
    this.emit('message', { version: 2, requestId, ok: true, result });
  }
}

const options = {
  modelId: 'Xenova/whisper-small' as const,
  sampleRate: 16_000 as const,
  language: 'en' as const,
};
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

  it('copies caller-owned PCM before asynchronous transcription work', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const pcm = Float32Array.from([0.1, 0.2]);
    const expected = [...pcm];
    const transcription = client.transcribe(pcm, options, new AbortController().signal);
    pcm.fill(9);

    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    const request = WhisperWorkerRequestSchema.parse(workers[0]?.messages.at(-1));
    if (request.type !== 'transcribe') throw new Error('Expected transcription request');
    expect(request.pcm).not.toBe(pcm.buffer);
    expect([...new Float32Array(request.pcm)]).toEqual(expected);
    workers[0]?.replyTranscription('copied');
    await transcription;
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

  it('cancels an idle session in-band and keeps the warm worker generation', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    const cancellation = session.cancel();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    expect(workers[0]?.killed).toBe(false);
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;

    const transcription = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    expect(workers).toHaveLength(1);
    workers[0]?.replyTranscription('warm');
    await expect(transcription).resolves.toMatchObject({ text: 'warm' });
    await client.close();
  });

  it('keeps only one streaming push in IPC flight at a time', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    const first = session.push(new Float32Array([0.1]));
    const second = session.push(new Float32Array([0.2]));
    await vi.waitFor(() => expect(requestCount(workers[0], 'session-push')).toBe(1));
    workers[0]?.replyAcknowledged('session-push');
    await first;
    await vi.waitFor(() => expect(requestCount(workers[0], 'session-push')).toBe(2));
    workers[0]?.replyAcknowledged('session-push');
    await second;

    const cancellation = session.cancel();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;
    await client.close();
  });

  it('copies queued PCM before returning control to its caller', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    const first = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(requestCount(workers[0], 'session-push')).toBe(1));
    const queuedPcm = Float32Array.from([0.2, 0.3]);
    const second = session.push(queuedPcm);
    queuedPcm.fill(9);

    workers[0]?.replyAcknowledged('session-push');
    await first;
    await vi.waitFor(() => expect(requestCount(workers[0], 'session-push')).toBe(2));
    const queuedRequest = WhisperWorkerRequestSchema.parse(workers[0]?.messages.at(-1));
    if (queuedRequest.type !== 'session-push') throw new Error('Expected queued push request');
    expect([...new Float32Array(queuedRequest.pcm)]).toEqual([...Float32Array.from([0.2, 0.3])]);
    workers[0]?.replyAcknowledged('session-push');
    await second;

    const cancellation = session.cancel();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;
    await client.close();
  });

  it('cancels the worker-side session when finish is aborted before dispatch', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const signal = AbortSignal.abort();
    const finishing = session.finish(signal);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    workers[0]?.replyAcknowledged('session-cancel');
    await expect(finishing).rejects.toMatchObject({ code: 'CANCELLED' });
    await client.close();
  });

  it('latches the first push failure, rejects queued work, and cancels the session', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    const capture = (operation: Promise<unknown>) =>
      operation.then(
        () => undefined,
        (error: unknown) => error,
      );
    const first = capture(session.push(new Float32Array([0.1])));
    const queued = capture(session.push(new Float32Array([0.2])));
    await vi.waitFor(() => expect(requestCount(workers[0], 'session-push')).toBe(1));
    workers[0]?.replyFailure('session-push', 'INFERENCE_FAILED', 'first push failed');

    const firstError = await first;
    expect(firstError).toMatchObject({ code: 'INFERENCE_FAILED', message: 'first push failed' });
    await expect(queued).resolves.toBe(firstError);
    expect(requestCount(workers[0], 'session-push')).toBe(1);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));

    await expect(capture(session.push(new Float32Array([0.3])))).resolves.toBe(firstError);
    await expect(capture(session.finish())).resolves.toBe(firstError);
    workers[0]?.replyAcknowledged('session-cancel');
    await session.cancel();
    await client.close();
  });

  it('retains the model lease until push-failure cleanup settles during finish', async () => {
    const workers: FakeWorker[] = [];
    const release = vi.fn();
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: async (modelId) => ({ ...(await readyAcquirer(modelId)), release }),
      spawn: () => {
        const worker = new FakeWorker();
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    const finishing = session.finish();
    const pushFailure = expect(push).rejects.toMatchObject({ code: 'INFERENCE_FAILED' });
    const finishFailure = expect(finishing).rejects.toMatchObject({ code: 'INFERENCE_FAILED' });
    workers[0]?.replyFailure('session-push', 'INFERENCE_FAILED', 'push failed during finish');

    await Promise.all([pushFailure, finishFailure]);
    expect(release).not.toHaveBeenCalled();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    expect(release).not.toHaveBeenCalled();
    workers[0]?.replyAcknowledged('session-cancel');
    await session.cancel();
    expect(release).toHaveBeenCalledOnce();
    await client.close();
  });

  it('falls back to process termination when in-band cancellation is unresponsive', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const opening = client.startSession(options);
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('session-open');
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;

    let settled = false;
    const cancellation = session.cancel().then(() => {
      settled = true;
    });
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('session-cancel');
    await vi.advanceTimersByTimeAsync(30_000);
    expect(workers[0]?.killed).toBe(true);
    expect(settled).toBe(false);
    workers[0]?.exit(1);
    await cancellation;
    expect(settled).toBe(true);
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

  it('uses an audio-duration-aware deadline for long one-shot inference', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    let settled = false;
    const transcription = client
      .transcribe(new Float32Array(WHISPER_SAMPLE_RATE * 60), options, new AbortController().signal)
      .finally(() => {
        settled = true;
      });
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('transcribe');

    await vi.advanceTimersByTimeAsync(120_000);
    expect(settled).toBe(false);
    expect(workers[0]?.killed).toBe(false);
    workers[0]?.replyTranscription('long result');
    await expect(transcription).resolves.toMatchObject({ text: 'long result' });
    await client.close();
  });

  it('uses buffered audio duration for streaming finish inference', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const pushing = session.push(new Float32Array([0.1]));
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    workers[0]?.replyAcknowledged('session-push');
    await pushing;

    let settled = false;
    const finishing = session.finish().finally(() => {
      settled = true;
    });
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('session-finish');
    await vi.advanceTimersByTimeAsync(120_000);
    expect(settled).toBe(false);
    expect(workers[0]?.killed).toBe(false);
    workers[0]?.replyTranscription('streamed result');
    await expect(finishing).resolves.toMatchObject({ text: 'streamed result' });
    await client.close();
  });

  it('starts short control deadlines only after earlier inference leaves the dispatch queue', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const transcription = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('transcribe');

    let controlSettled = false;
    const pressure = client.memoryPressure().finally(() => {
      controlSettled = true;
    });
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(requestCount(workers[0], 'memory-pressure')).toBe(0);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(controlSettled).toBe(false);
    expect(workers[0]?.killed).toBe(false);

    workers[0]?.replyTranscription('first complete');
    await transcription;
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('memory-pressure');
    workers[0]?.replyAcknowledged('memory-pressure');
    await pressure;
    await client.close();
  });

  it('cancels a queued request without terminating active inference', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const active = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    const queuedController = new AbortController();
    const queued = client.transcribe(new Float32Array([0.2]), options, queuedController.signal);
    await vi.waitFor(() => expect(requestCount(workers[0], 'transcribe')).toBe(1));
    const queuedFailure = expect(queued).rejects.toMatchObject({ code: 'CANCELLED' });
    queuedController.abort();
    await queuedFailure;
    expect(workers[0]?.killed).toBe(false);
    expect(requestCount(workers[0], 'transcribe')).toBe(1);

    workers[0]?.replyTranscription('active complete');
    await expect(active).resolves.toMatchObject({ text: 'active complete' });
    await client.close();
  });

  it('does not dispatch a queued request into a replacement worker generation', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const active = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    const queued = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(requestCount(workers[0], 'transcribe')).toBe(1));
    const activeFailure = expect(active).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    const queuedFailure = expect(queued).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    workers[0]?.exit(1);
    await Promise.all([activeFailure, queuedFailure]);
    expect(requestCount(workers[0], 'transcribe')).toBe(1);
    await client.close();
  });

  it('does not let queued session cancellation time out unrelated inference', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const unrelated = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('transcribe');

    const cancellation = session.cancel();
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(requestCount(workers[0], 'session-cancel')).toBe(0);
    await vi.advanceTimersByTimeAsync(30_000);
    expect(workers[0]?.killed).toBe(false);

    workers[0]?.replyTranscription('unrelated complete');
    await unrelated;
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('session-cancel');
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;
    await client.close();
  });

  it('cancels a never-dispatched session push without killing unrelated inference', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const unrelated = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    const push = session.push(new Float32Array([0.2]));
    await Promise.resolve();
    expect(requestCount(workers[0], 'session-push')).toBe(0);

    const pushFailure = expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    const cancellation = session.cancel();
    await pushFailure;
    expect(workers[0]?.killed).toBe(false);
    workers[0]?.replyTranscription('unrelated complete');
    await unrelated;
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;
    await client.close();
  });

  it('observes a push signal while that push is waiting on the session-local queue', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const active = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    const controller = new AbortController();
    const queued = session.push(new Float32Array([0.2]), controller.signal);
    const queuedFailure = expect(queued).rejects.toMatchObject({ code: 'CANCELLED' });
    controller.abort();
    await queuedFailure;
    expect(workers[0]?.killed).toBe(false);
    expect(requestCount(workers[0], 'session-push')).toBe(1);

    workers[0]?.replyAcknowledged('session-push');
    await active;
    const cancellation = session.cancel();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    workers[0]?.replyAcknowledged('session-cancel');
    await cancellation;
    await client.close();
  });

  it('observes finish cancellation while a push is still active', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    const controller = new AbortController();
    const pushFailure = expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    const finishFailure = expect(session.finish(controller.signal)).rejects.toMatchObject({
      code: 'CANCELLED',
    });
    controller.abort();
    await Promise.all([pushFailure, finishFailure]);
    expect(workers[0]?.killed).toBe(true);
    await client.close();
  });

  it('lets session.cancel preempt an in-flight streaming finish', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const pushing = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    workers[0]?.replyAcknowledged('session-push');
    await pushing;
    const finishing = session.finish();
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-finish'));
    const finishFailure = expect(finishing).rejects.toMatchObject({ code: 'CANCELLED' });
    await session.cancel();
    await finishFailure;
    expect(workers[0]?.killed).toBe(true);
    await client.close();
  });

  it('deduplicates worker-side cancellation when finish aborts a globally queued push', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const unrelated = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    const push = session.push(new Float32Array([0.2]));
    await Promise.resolve();
    const finishController = new AbortController();
    const pushFailure = expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    const finishFailure = expect(session.finish(finishController.signal)).rejects.toMatchObject({
      code: 'CANCELLED',
    });
    finishController.abort();
    await Promise.all([pushFailure, finishFailure]);
    expect(workers[0]?.killed).toBe(false);

    workers[0]?.replyTranscription('unrelated complete');
    await unrelated;
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-cancel'));
    expect(requestCount(workers[0], 'session-cancel')).toBe(1);
    workers[0]?.replyAcknowledged('session-cancel');
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    await client.close();
  });

  it('bounds retained one-shot PCM before copying another queued request', async () => {
    const workers: FakeWorker[] = [];
    const acquire = vi.fn(readyAcquirer);
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      maxConcurrentPcmSamples: 2,
      acquireModelUse: acquire,
      spawn: () => {
        const worker = new FakeWorker();
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    const active = client.transcribe(
      new Float32Array([0.1, 0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    await expect(
      client.transcribe(new Float32Array([0.3]), options, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_AUDIO' });
    expect(acquire).toHaveBeenCalledOnce();
    workers[0]?.replyTranscription('within bound');
    await active;
    await client.close();
  });

  it('rejects empty PCM before acquiring a model or spawning a worker', async () => {
    const acquire = vi.fn<ModelUseAcquirer>();
    const spawn = vi.fn<WhisperWorkerSpawner>();
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: acquire,
      spawn,
    });
    await expect(
      client.transcribe(new Float32Array(), options, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'INVALID_AUDIO' });
    expect(acquire).not.toHaveBeenCalled();
    expect(spawn).not.toHaveBeenCalled();
  });

  it('reports cancellation only to its initiating request and crashes collateral work', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const controller = new AbortController();
    const cancelled = client.transcribe(new Float32Array([0.1]), options, controller.signal);
    const collateral = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(requestCount(workers[0], 'transcribe')).toBe(1));
    controller.abort();
    await expect(cancelled).rejects.toMatchObject({ code: 'CANCELLED' });
    await expect(collateral).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    await client.close();
  });

  it('rejects a session-open acknowledgement racing with worker exit and releases its lease', async () => {
    const workers: FakeWorker[] = [];
    const release = vi.fn();
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      spawn: () => {
        const worker = new FakeWorker();
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
      acquireModelUse: (modelId) =>
        Promise.resolve({
          status: {
            modelId,
            state: 'ready',
            downloadedBytes: 1,
            totalBytes: 1,
            detail: null,
            repairable: false,
          },
          release,
        }),
    });
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    workers[0]?.exit(1);
    await expect(opening).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    expect(release).toHaveBeenCalledOnce();
    await client.close();
  });

  it('shares concurrent streaming cancellation completion', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-push'));
    const collateral = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await Promise.resolve();
    expect(requestCount(workers[0], 'transcribe')).toBe(0);

    const first = session.cancel();
    let secondSettled = false;
    const second = session.cancel().finally(() => {
      secondSettled = true;
    });
    await Promise.resolve();
    expect(secondSettled).toBe(false);
    workers[0]?.exit(1);
    await Promise.all([first, second]);
    await expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    await expect(collateral).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    await client.close();
  });

  it('retains a streaming model lease until an unkillable worker actually exits', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const release = vi.fn();
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: async (modelId) => ({ ...(await readyAcquirer(modelId)), release }),
      spawn: () => {
        const worker = new FakeWorker();
        worker.autoExitOnKill = false;
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    const opening = client.startSession(options);
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    workers[0]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    const worker = workers[0];
    if (worker === undefined) throw new Error('Worker missing');
    vi.spyOn(worker, 'kill').mockImplementation(() => {
      throw new Error('kill failed');
    });
    const pushFailure = expect(push).rejects.toMatchObject({ code: 'CANCELLED' });
    const cancellation = session.cancel();
    await vi.advanceTimersByTimeAsync(4_500);
    await Promise.all([pushFailure, cancellation]);
    expect(release).not.toHaveBeenCalled();
    await expect(client.close()).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    worker.exit(1);
    expect(release).toHaveBeenCalledOnce();
  });

  it('releases a cancelled never-dispatched lease during unkillable termination', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const releases = [vi.fn(), vi.fn()];
    let acquisitions = 0;
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: async (modelId) => ({
        ...(await readyAcquirer(modelId)),
        release: releases[acquisitions++] ?? vi.fn(),
      }),
      spawn: () => {
        const worker = new FakeWorker();
        worker.autoExitOnKill = false;
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    const activeController = new AbortController();
    const queuedController = new AbortController();
    const active = client.transcribe(new Float32Array([0.1]), options, activeController.signal);
    const queued = client.transcribe(new Float32Array([0.2]), options, queuedController.signal);
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    const worker = workers[0];
    if (worker === undefined) throw new Error('Worker missing');
    expect(requestCount(worker, 'transcribe')).toBe(1);
    expect(acquisitions).toBe(2);
    vi.spyOn(worker, 'kill').mockImplementation(() => {
      throw new Error('kill failed');
    });
    const activeFailure = expect(active).rejects.toMatchObject({ code: 'CANCELLED' });
    const queuedFailure = expect(queued).rejects.toMatchObject({ code: 'CANCELLED' });
    activeController.abort();
    queuedController.abort();
    await queuedFailure;
    expect(releases[1]).toHaveBeenCalledOnce();
    expect(releases[0]).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(4_500);
    await activeFailure;
    expect(releases[0]).not.toHaveBeenCalled();
    worker.exit(1);
    expect(releases[0]).toHaveBeenCalledOnce();
  });

  it('lets cancellation interrupt an existing termination barrier', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers, false);
    const activeController = new AbortController();
    const active = client
      .transcribe(new Float32Array([0.1]), options, activeController.signal)
      .catch((error: unknown) => error);
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    const worker = workers[0];
    if (worker === undefined) throw new Error('Worker missing');
    vi.spyOn(worker, 'kill').mockImplementation(() => {
      throw new Error('kill failed');
    });
    activeController.abort();

    const barrierController = new AbortController();
    const blocked = client.checkWorkerModel('Xenova/whisper-small', barrierController.signal);
    const blockedFailure = expect(blocked).rejects.toMatchObject({ code: 'CANCELLED' });
    barrierController.abort();
    await blockedFailure;
    await vi.advanceTimersByTimeAsync(4_500);
    await expect(active).resolves.toMatchObject({ code: 'CANCELLED' });
    worker.exit(1);
  });

  it('bounds unresponsive termination even when process.kill throws', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const release = vi.fn();
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: async (modelId) => ({ ...(await readyAcquirer(modelId)), release }),
      spawn: () => {
        const worker = new FakeWorker();
        worker.autoExitOnKill = false;
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    const controller = new AbortController();
    const request = client.transcribe(new Float32Array([0.1]), options, controller.signal).then(
      () => null,
      (error: unknown) => error,
    );
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('transcribe');
    const worker = workers[0];
    if (worker === undefined) throw new Error('Worker missing');
    vi.spyOn(worker, 'kill').mockImplementation(() => {
      throw new Error('kill failed');
    });
    controller.abort();
    await vi.advanceTimersByTimeAsync(4_500);
    await expect(request).resolves.toMatchObject({ code: 'CANCELLED' });
    expect(release).not.toHaveBeenCalled();
    await expect(client.close()).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    worker.exit(1);
    expect(release).toHaveBeenCalledOnce();
  });

  it('times out a healthy worker that ignores an operation after readiness', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const warmup = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    workers[0]?.replyTranscription('warm');
    await warmup;
    const pressure = client.memoryPressure().then(
      () => null,
      (error: unknown) => error,
    );
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    expect(latestRequestType(workers[0])).toBe('memory-pressure');
    await vi.advanceTimersByTimeAsync(30_000);
    await expect(pressure).resolves.toMatchObject({ code: 'WORKER_CRASHED' });
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

  it('does not spawn a replacement if the captured worker exits while close queues shutdown', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const warmup = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('transcribe'));
    workers[0]?.replyTranscription('warm');
    await warmup;
    const closing = client.close();
    workers[0]?.exit(0);
    await closing;
    expect(workers).toHaveLength(1);
  });

  it('classifies active and queued requests as cancelled during intentional close', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    const client = createClient(workers);
    const active = client.transcribe(
      new Float32Array([0.1]),
      options,
      new AbortController().signal,
    );
    const queued = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 10; flush += 1) await Promise.resolve();
    expect(requestCount(workers[0], 'transcribe')).toBe(1);
    const activeFailure = expect(active).rejects.toMatchObject({ code: 'CANCELLED' });
    const queuedFailure = expect(queued).rejects.toMatchObject({ code: 'CANCELLED' });
    const closing = client.close();
    await vi.advanceTimersByTimeAsync(1_100);
    await Promise.all([activeFailure, queuedFailure, closing]);
    expect(workers).toHaveLength(1);
  });

  it('rejects mismatched acknowledgements for open, push, pressure, and shutdown', async () => {
    const workers: FakeWorker[] = [];
    const client = createClient(workers);

    const badOpen = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[0])).toBe('session-open'));
    workers[0]?.replyAcknowledgementFor('session-open', 'memory-pressure');
    await expect(badOpen).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const opening = client.startSession(options);
    await vi.waitFor(() => expect(latestRequestType(workers[1])).toBe('session-open'));
    workers[1]?.replyAcknowledged('session-open');
    const session = await opening;
    const push = session.push(new Float32Array([0.1]));
    await vi.waitFor(() => expect(latestRequestType(workers[1])).toBe('session-push'));
    workers[1]?.replyAcknowledgementFor('session-push', 'unload');
    await expect(push).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const replacement = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[2])).toBe('transcribe'));
    workers[2]?.replyTranscription('replacement');
    await replacement;
    const pressure = client.memoryPressure();
    await vi.waitFor(() => expect(latestRequestType(workers[2])).toBe('memory-pressure'));
    workers[2]?.replyAcknowledgementFor('memory-pressure', 'unload');
    await expect(pressure).rejects.toMatchObject({ code: 'PROTOCOL_ERROR' });

    const shutdownWorker = client.transcribe(
      new Float32Array([0.3]),
      options,
      new AbortController().signal,
    );
    await vi.waitFor(() => expect(latestRequestType(workers[3])).toBe('transcribe'));
    workers[3]?.replyTranscription('shutdown worker');
    await shutdownWorker;
    const worker = workers[3];
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

  it('supervises a transient initial spawn failure with the bounded restart policy', async () => {
    vi.useFakeTimers();
    const workers: FakeWorker[] = [];
    let attempts = 0;
    const client = new WhisperWorkerClient({
      cacheDirectory: 'models',
      workerPath: 'worker.js',
      acquireModelUse: readyAcquirer,
      spawn: () => {
        attempts += 1;
        if (attempts === 1) throw new Error('transient spawn failure');
        const worker = new FakeWorker();
        workers.push(worker);
        queueMicrotask(() => worker.ready());
        return worker;
      },
    });
    await expect(
      client.transcribe(new Float32Array([0.1]), options, new AbortController().signal),
    ).rejects.toMatchObject({ code: 'WORKER_CRASHED' });
    await vi.advanceTimersByTimeAsync(100);
    expect(workers).toHaveLength(1);
    const recovered = client.transcribe(
      new Float32Array([0.2]),
      options,
      new AbortController().signal,
    );
    for (let flush = 0; flush < 5; flush += 1) await Promise.resolve();
    workers[0]?.replyTranscription('recovered');
    await expect(recovered).resolves.toMatchObject({ text: 'recovered' });
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

function requestCount(worker: FakeWorker | undefined, type: string): number {
  return (
    worker?.messages.filter((message) => WhisperWorkerRequestSchema.parse(message).type === type)
      .length ?? 0
  );
}
