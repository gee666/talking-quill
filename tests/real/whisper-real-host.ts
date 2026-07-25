import { app, utilityProcess } from 'electron';
import { createReadStream } from 'node:fs';
import { mkdir, readFile, readdir, rm, stat, writeFile } from 'node:fs/promises';
import { createServer, type Server } from 'node:http';
import { dirname, join, resolve } from 'node:path';
import { performance } from 'node:perf_hooks';
import { WhisperClientError } from '../../app/src/main/transcription/errors';
import { ModelManager } from '../../app/src/main/transcription/model-manager';
import { MODEL_MANIFEST } from '../../app/src/main/transcription/model-manifest';
import { WhisperWorkerClient } from '../../app/src/main/transcription/whisper-worker-client';
import { parsePcm16Wav } from '../helpers/wav';

const MODEL_ID = 'Xenova/whisper-small' as const;
const CACHE_ROOT = resolve('tmp', 'whisper-real-cache');
const SEED_MODELS_DIRECTORY = join(CACHE_ROOT, 'models');
const RUN_ROOT = resolve('tmp', 'whisper-real-run');
const RUN_MODELS_DIRECTORY = join(RUN_ROOT, 'models');
const SHORT_FIXTURE = resolve('tests', 'fixtures', 'audio', 'speech-short-16k-mono.wav');
const BOUNDARY_FIXTURE = resolve(
  'tests',
  'fixtures',
  'audio',
  'speech-boundaries-90s-16k-mono.wav',
);
const WARM_RUNS = 3;
const TEST_IDLE_UNLOAD_MS = 1_000;
const TEST_TIMEOUT_MS = 30 * 60_000;

void app.whenReady().then(async () => {
  const timeout = setTimeout(() => {
    console.error('Real Whisper test exceeded its 30-minute timeout.');
    app.exit(1);
  }, TEST_TIMEOUT_MS);
  let client: WhisperWorkerClient | null = null;
  let seedManager: ModelManager | null = null;
  let manager: ModelManager | null = null;
  let rangeServer: LocalRangeServer | null = null;
  try {
    seedManager = new ModelManager({
      modelsDirectory: SEED_MODELS_DIRECTORY,
      temporaryDirectory: join(SEED_MODELS_DIRECTORY, '.tmp'),
    });
    await seedManager.initialize();
    const initial = await seedManager.status(MODEL_ID, false);
    if (initial.state !== 'ready') {
      console.log(`Whisper model state is ${initial.state}; downloading into ${CACHE_ROOT}`);
      let lastReportedPercent = -10;
      seedManager.subscribe((event) => {
        if (event.modelId !== MODEL_ID) return;
        const percent = Math.floor((event.total.downloadedBytes / event.total.totalBytes) * 100);
        const reportablePercent = Math.floor(percent / 10) * 10;
        if (reportablePercent > lastReportedPercent) {
          lastReportedPercent = reportablePercent;
          process.stdout.write(`model-download ${String(reportablePercent)}%\r`);
        }
      });
      await seedManager.download(MODEL_ID);
      process.stdout.write('model-download 100%\n');
    }

    await rm(RUN_ROOT, { recursive: true, force: true });
    rangeServer = await startLocalRangeServer(modelSourceDirectory());
    manager = new ModelManager({
      modelsDirectory: RUN_MODELS_DIRECTORY,
      temporaryDirectory: join(RUN_MODELS_DIRECTORY, '.tmp'),
      urlFor: (_model, file) => rangeServer?.urlFor(file.path) ?? 'http://127.0.0.1/closed',
      validateRequestUrl: (url) => url.startsWith('http://127.0.0.1:'),
    });
    await manager.initialize();
    await preseedResumablePart();
    await manager.download(MODEL_ID);
    if (rangeServer.rangedRequests < 1) {
      throw new Error('Production ModelManager did not resume through the local Range server.');
    }
    client = new WhisperWorkerClient({
      cacheDirectory: RUN_MODELS_DIRECTORY,
      workerPath: resolve('app', 'out', 'workers', 'whisper-bootstrap.cjs'),
      spawn: (modulePath, arguments_) =>
        utilityProcess.fork(
          modulePath,
          [
            ...arguments_,
            '--network-guard-probe',
            `--idle-unload-ms=${String(TEST_IDLE_UNLOAD_MS)}`,
          ],
          {
            serviceName: 'Talking Quill Whisper Guarded Real Test',
            stdio: 'ignore',
          },
        ),
      acquireModelUse: (modelId, signal) =>
        manager?.acquireUse(modelId, signal) ??
        Promise.reject(new Error('Model manager unavailable')),
    });
    manager.setBeforeMutation((modelId) => client?.unload(modelId) ?? Promise.resolve());

    const short = parsePcm16Wav(await readFile(SHORT_FIXTURE));
    const measured = [];
    for (let index = 0; index < 1 + WARM_RUNS; index += 1) {
      const startedAt = performance.now();
      const result = await client.transcribe(
        short.pcm,
        { modelId: MODEL_ID, sampleRate: 16_000, language: 'en' },
        new AbortController().signal,
      );
      measured.push({ result, elapsedMs: performance.now() - startedAt });
      assertShortContent(result.text);
    }
    const cold = measured[0];
    if (cold === undefined) throw new Error('Cold transcription result missing');
    if (cold.result.pipeline.reused || cold.result.pipeline.loadCount !== 1) {
      throw new Error(
        `Cold pipeline metadata was invalid: ${JSON.stringify(cold.result.pipeline)}`,
      );
    }
    for (const warm of measured.slice(1)) {
      if (!warm.result.pipeline.reused || warm.result.pipeline.loadCount !== 1) {
        throw new Error(
          `Warm pipeline metadata was invalid: ${JSON.stringify(warm.result.pipeline)}`,
        );
      }
    }
    const warmMedian = median(measured.slice(1).map((entry) => entry.elapsedMs));
    if (warmMedian >= cold.elapsedMs) {
      throw new Error(
        `Warm median must be strictly faster: cold=${String(cold.elapsedMs)}, warm=${String(warmMedian)}`,
      );
    }

    const boundary = parsePcm16Wav(await readFile(BOUNDARY_FIXTURE));
    const session = await client.startSession({
      modelId: MODEL_ID,
      sampleRate: 16_000,
      language: 'en',
    });
    const pushSamples = 10 * 16_000;
    for (let offset = 0; offset < boundary.pcm.length; offset += pushSamples) {
      await session.push(
        boundary.pcm.subarray(offset, Math.min(offset + pushSamples, boundary.pcm.length)),
      );
    }
    const streamed = await session.finish();
    const normalizedBoundary = normalize(streamed.text);
    for (const anchor of [
      'alpha boundary marker',
      'bravo boundary marker',
      'charlie boundary marker',
      'delta boundary marker',
    ]) {
      const count = countPhrase(normalizedBoundary, anchor);
      if (count !== 1) {
        throw new Error(
          `Expected boundary anchor exactly once (${anchor}), count=${String(count)}: ${streamed.text}`,
        );
      }
    }
    if (!streamed.pipeline.reused || streamed.pipeline.loadCount !== 1) {
      throw new Error(
        `Streaming did not reuse the production pipeline: ${JSON.stringify(streamed.pipeline)}`,
      );
    }

    await delay(TEST_IDLE_UNLOAD_MS + 500);
    const afterIdle = await client.transcribe(
      short.pcm,
      { modelId: MODEL_ID, sampleRate: 16_000, language: 'en' },
      new AbortController().signal,
    );
    assertShortContent(afterIdle.text);
    if (afterIdle.pipeline.reused || afterIdle.pipeline.loadCount !== 2) {
      throw new Error(
        `Pipeline did not unload and reload after idle: ${JSON.stringify(afterIdle.pipeline)}`,
      );
    }

    const cancellation = new AbortController();
    const cancelledTranscription = client.transcribe(
      boundary.pcm,
      { modelId: MODEL_ID, sampleRate: 16_000, language: 'en' },
      cancellation.signal,
    );
    setTimeout(() => cancellation.abort('real integration cancellation'), 50);
    try {
      await cancelledTranscription;
      throw new Error('Active production inference completed after cancellation');
    } catch (error: unknown) {
      if (!(error instanceof WhisperClientError) || error.code !== 'CANCELLED') throw error;
    }
    const afterCancellation = await client.transcribe(
      short.pcm,
      { modelId: MODEL_ID, sampleRate: 16_000, language: 'en' },
      new AbortController().signal,
    );
    assertShortContent(afterCancellation.text);
    if (afterCancellation.pipeline.reused || afterCancellation.pipeline.loadCount !== 1) {
      throw new Error(
        `Worker did not recover cold after cancellation: ${JSON.stringify(afterCancellation.pipeline)}`,
      );
    }

    const cacheBytes = await directoryBytes(CACHE_ROOT);
    console.log(
      `Real q8 Whisper passed: cold=${cold.elapsedMs.toFixed(0)}ms warm-median=${warmMedian.toFixed(0)}ms load-count=1 range-requests=${String(rangeServer.rangedRequests)} cache=${CACHE_ROOT} bytes=${String(cacheBytes)}`,
    );
    console.log(`Boundary transcript: ${streamed.text}`);
    clearTimeout(timeout);
    await client.close();
    await manager.shutdown();
    await seedManager.shutdown();
    await rangeServer.close();
    await rm(RUN_ROOT, { recursive: true, force: true });
    app.exit(0);
  } catch (error: unknown) {
    clearTimeout(timeout);
    await Promise.allSettled([
      client?.close(),
      manager?.shutdown(),
      seedManager?.shutdown(),
      rangeServer?.close(),
    ]);
    await rm(RUN_ROOT, { recursive: true, force: true }).catch(() => undefined);
    console.error(error);
    app.exit(1);
  }
});

interface LocalRangeServer {
  readonly rangedRequests: number;
  urlFor(filePath: string): string;
  close(): Promise<void>;
}

const SMALL_MODEL = requireSmallModel();

function requireSmallModel() {
  const model = MODEL_MANIFEST.models.find((candidate) => candidate.id === MODEL_ID);
  if (model === undefined) throw new Error('Pinned small model manifest missing');
  return model;
}

function modelSourceDirectory(): string {
  return join(SEED_MODELS_DIRECTORY, MODEL_ID, SMALL_MODEL.revision);
}

async function preseedResumablePart(): Promise<void> {
  const file = SMALL_MODEL.files[0];
  if (file === undefined) throw new Error('Pinned model has no files');
  const source = await readFile(join(modelSourceDirectory(), file.path));
  const part = join(
    RUN_MODELS_DIRECTORY,
    '.tmp',
    MODEL_ID,
    SMALL_MODEL.revision,
    `${file.path}.part`,
  );
  await mkdir(dirname(part), { recursive: true });
  await writeFile(part, source.subarray(0, Math.max(1, Math.floor(source.length / 2))));
}

async function startLocalRangeServer(sourceDirectory: string): Promise<LocalRangeServer> {
  const routes = new Map(
    SMALL_MODEL.files.map((file) => [
      `/artifact/${file.path.split('/').map(encodeURIComponent).join('/')}`,
      join(sourceDirectory, file.path),
    ]),
  );
  let rangedRequests = 0;
  const server = createServer((request, response) => {
    void (async () => {
      const pathname = new URL(request.url ?? '/', 'http://127.0.0.1').pathname;
      const source = routes.get(pathname);
      if (source === undefined) {
        response.writeHead(404).end();
        return;
      }
      const size = (await stat(source)).size;
      const range = /^bytes=(\d+)-$/.exec(request.headers.range ?? '');
      let start = 0;
      let status = 200;
      if (range !== null) {
        rangedRequests += 1;
        start = Number(range[1]);
        if (!Number.isSafeInteger(start) || start >= size) {
          response.writeHead(416, { 'content-range': `bytes */${String(size)}` }).end();
          return;
        }
        status = 206;
      }
      response.writeHead(status, {
        'accept-ranges': 'bytes',
        'content-length': String(size - start),
        'content-type': 'application/octet-stream',
        ...(status === 206
          ? { 'content-range': `bytes ${String(start)}-${String(size - 1)}/${String(size)}` }
          : {}),
      });
      createReadStream(source, { start }).pipe(response);
    })().catch((error: unknown) => {
      response.destroy(error instanceof Error ? error : new Error('Local Range server failed'));
    });
  });
  await new Promise<void>((resolveListen, rejectListen) => {
    server.once('error', rejectListen);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', rejectListen);
      resolveListen();
    });
  });
  const address = server.address();
  if (address === null || typeof address === 'string')
    throw new Error('Range server address missing');
  const origin = `http://127.0.0.1:${String(address.port)}`;
  return {
    get rangedRequests() {
      return rangedRequests;
    },
    urlFor(filePath) {
      return `${origin}/artifact/${filePath.split('/').map(encodeURIComponent).join('/')}`;
    },
    close: () => closeServer(server),
  };
}

function closeServer(server: Server): Promise<void> {
  return new Promise((resolveClose, rejectClose) => {
    server.close((error) => (error === undefined ? resolveClose() : rejectClose(error)));
  });
}

function assertShortContent(text: string): void {
  const normalized = normalize(text);
  for (const phrase of ['talking quill', 'private', 'local']) {
    if (!normalized.includes(phrase)) {
      throw new Error(`Short transcript omitted ${phrase}: ${text}`);
    }
  }
}

function normalize(value: string): string {
  return value
    .normalize('NFKD')
    .replace(/\p{Mark}|[^\p{Letter}\p{Number}]+/gu, ' ')
    .trim()
    .toLowerCase();
}

function countPhrase(text: string, phrase: string): number {
  let count = 0;
  let offset = 0;
  for (;;) {
    const found = text.indexOf(phrase, offset);
    if (found < 0) return count;
    count += 1;
    offset = found + phrase.length;
  }
}

function median(values: readonly number[]): number {
  if (values.length === 0) throw new Error('Median requires values');
  const ordered = [...values].sort((left, right) => left - right);
  const value = ordered[Math.floor(ordered.length / 2)];
  if (value === undefined) throw new Error('Median value missing');
  return value;
}

function delay(milliseconds: number): Promise<void> {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}

async function directoryBytes(path: string): Promise<number> {
  let entries;
  try {
    entries = await readdir(path, { withFileTypes: true });
  } catch {
    return 0;
  }
  let total = 0;
  for (const entry of entries) {
    const target = join(path, entry.name);
    if (entry.isDirectory()) total += await directoryBytes(target);
    else if (entry.isFile()) total += (await stat(target)).size;
  }
  return total;
}
