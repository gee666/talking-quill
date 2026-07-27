import { app, utilityProcess } from 'electron';
import { access, mkdir, rm } from 'node:fs/promises';
import { resolve } from 'node:path';
import { WhisperWorkerClient } from '../../app/src/main/transcription/whisper-worker-client';
import { WhisperClientError } from '../../app/src/main/transcription/errors';

const marker = resolve('tmp', 'tests', 'whisper-active.marker');
const crashMarker = resolve('tmp', 'tests', 'whisper-crash-active.marker');
const workerPath = resolve('tests', 'integration', 'whisper-controllable-worker.cjs');
const transcriptionOptions = {
  modelId: 'Xenova/whisper-small' as const,
  sampleRate: 16_000 as const,
  language: 'en' as const,
};
void app.whenReady().then(async () => {
  let order = 0;
  let firstExitOrder = 0;
  let generation = 0;
  const currentGeneration = () => generation;
  const workers: ReturnType<typeof utilityProcess.fork>[] = [];
  try {
    await mkdir(resolve('tmp', 'tests'), { recursive: true });
    await Promise.all([rm(marker, { force: true }), rm(crashMarker, { force: true })]);
    const client = new WhisperWorkerClient({
      cacheDirectory: resolve('tmp', 'tests', 'unused-model-cache'),
      workerPath,
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
          release: () => undefined,
        }),
      spawn: (modulePath, args) => {
        generation += 1;
        const currentGeneration = generation;
        const worker = utilityProcess.fork(
          modulePath,
          [
            ...args,
            `--test-generation=${String(currentGeneration)}`,
            `--test-marker=${marker}`,
            `--test-crash-marker=${crashMarker}`,
          ],
          { serviceName: 'Talking Quill Whisper Test', stdio: 'ignore' },
        );
        workers.push(worker);
        worker.on('exit', () => {
          if (currentGeneration === 1) firstExitOrder = ++order;
        });
        return worker;
      },
    });

    const controller = new AbortController();
    const active = client.transcribe(
      new Float32Array([0.1]),
      transcriptionOptions,
      controller.signal,
    );
    await waitForFile(marker, 10_000);
    controller.abort();
    let cancellation: unknown;
    try {
      await active;
    } catch (error: unknown) {
      cancellation = error;
    }
    const cancellationOrder = ++order;
    if (!(cancellation instanceof WhisperClientError) || cancellation.code !== 'CANCELLED') {
      throw new Error(`Expected CANCELLED, received ${String(cancellation)}`);
    }
    if (firstExitOrder === 0 || cancellationOrder <= firstExitOrder) {
      throw new Error('Cancellation settled before the real utility-process exit event.');
    }

    const replacement = await client.transcribe(
      new Float32Array([0.2]),
      transcriptionOptions,
      new AbortController().signal,
    );
    if (replacement.text !== 'healthy replacement' || currentGeneration() !== 2) {
      throw new Error(`Replacement generation failed: ${JSON.stringify(replacement)}`);
    }

    const crashingRequest = client.transcribe(
      new Float32Array([0.3]),
      transcriptionOptions,
      new AbortController().signal,
    );
    await waitForFile(crashMarker, 10_000);
    const activeWorker = workers[1];
    if (activeWorker?.kill() !== true) {
      throw new Error('Unable to externally terminate the active utility process.');
    }
    await expectClientError(crashingRequest, 'WORKER_CRASHED');
    await waitForCondition(() => currentGeneration() >= 3, 10_000);
    const afterCrash = await client.transcribe(
      new Float32Array([0.4]),
      transcriptionOptions,
      new AbortController().signal,
    );
    if (afterCrash.text !== 'healthy replacement' || currentGeneration() !== 3) {
      throw new Error(`Supervised crash recovery failed: ${JSON.stringify(afterCrash)}`);
    }
    await client.close();
    console.log(
      'Real WhisperWorkerClient waited for cancellation exit and supervised a spontaneous utility-process crash',
    );
    app.exit(0);
  } catch (error: unknown) {
    console.error(error);
    app.exit(1);
  } finally {
    await Promise.all([rm(marker, { force: true }), rm(crashMarker, { force: true })]);
  }
});

async function expectClientError(
  operation: Promise<unknown>,
  code: WhisperClientError['code'],
): Promise<void> {
  try {
    await operation;
  } catch (error: unknown) {
    if (error instanceof WhisperClientError && error.code === code) return;
    throw error;
  }
  throw new Error(`Expected Whisper client error ${code}`);
}

async function waitForCondition(condition: () => boolean, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (condition()) return;
    await new Promise((resolveWait) => setTimeout(resolveWait, 20));
  }
  throw new Error('Timed out waiting for supervised replacement worker.');
}

async function waitForFile(path: string, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      await access(path);
      return;
    } catch {
      await new Promise((resolveWait) => setTimeout(resolveWait, 20));
    }
  }
  throw new Error('Timed out waiting for controllable worker activity.');
}
