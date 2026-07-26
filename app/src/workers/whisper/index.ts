import { freemem, totalmem } from 'node:os';
import rawManifest from '../../../../scripts/model-manifest.json';
import { WHISPER_MAX_SAMPLES } from '../../shared/constants/whisper';
import { ModelManifestSchema, type WhisperModelId } from '../../shared/schemas/model-manifest';
import {
  WhisperWorkerRequestSchema,
  WhisperWorkerResponseSchema,
  type WhisperWorkerErrorCode,
  type WhisperWorkerRequest,
  type WhisperWorkerResponse,
} from '../../shared/schemas/whisper-protocol';
import { WhisperRequestQueue } from './request-queue';
import { WhisperRuntime, type WhisperPipeline } from './runtime';
import { WorkerModelVerificationError, verifyModelFiles } from './verify-model';

const MAX_PENDING_WORKER_REQUESTS = 64;
const RESERVED_CONTROL_REQUESTS = 8;
const MAX_PENDING_PCM_BYTES = WHISPER_MAX_SAMPLES * Float32Array.BYTES_PER_ELEMENT;
const networkGuard = readNetworkGuardState();

void startWorker().catch(() => {
  process.stderr.write('Whisper worker startup failed.\n');
  process.exit(1);
});

async function startWorker(): Promise<void> {
  const { env, pipeline } = await import('@huggingface/transformers');
  const manifest = ModelManifestSchema.parse(rawManifest);
  const cacheDirectory = readRequiredArgument('--model-cache=');
  const idleUnloadMs = readOptionalPositiveIntegerArgument('--idle-unload-ms=');
  const revisions = Object.fromEntries(
    manifest.models.map((model) => [model.id, model.revision]),
  ) as Record<(typeof manifest.models)[number]['id'], string>;
  const models = new Map(manifest.models.map((model) => [model.id, model]));

  env.allowRemoteModels = false;
  env.allowLocalModels = true;
  env.useFSCache = true;
  env.cacheDir = cacheDirectory;

  const runtime = new WhisperRuntime({
    cacheDirectory,
    revisions,
    ...(idleUnloadMs === undefined ? {} : { idleUnloadMs }),
    verify: async (modelId, revision, cache) => {
      await verifyModelFiles(cache, readManifestModel(models, modelId, revision));
    },
    factory: async (modelId, revision, cache) => {
      const model = readManifestModel(models, modelId, revision);
      const loaded: unknown = await pipeline('automatic-speech-recognition', modelId, {
        cache_dir: cache,
        revision,
        dtype: model.dtype,
        local_files_only: true,
      });
      if (typeof loaded !== 'function') throw new Error('Whisper pipeline did not load.');
      return loaded as WhisperPipeline;
    },
  });

  const pressureTimer = setInterval(() => {
    if (totalmem() > 0 && freemem() / totalmem() < 0.08) void runtime.memoryPressure();
  }, 30_000);
  pressureTimer.unref();

  const deferredSessionCleanup = new Set<string>();
  let sessionCleanupScheduled = false;
  const queue = new WhisperRequestQueue({
    maximumRequests: MAX_PENDING_WORKER_REQUESTS,
    maximumControlRequests: RESERVED_CONTROL_REQUESTS,
    maximumPcmBytes: MAX_PENDING_PCM_BYTES,
    execute: (request) =>
      handleRequest(request, runtime, models, pressureTimer, networkGuard.probeCompleted),
    rejectOverload: (request) => {
      if (request.type === 'session-cancel' || request.type === 'session-finish') {
        runtime.cancelSession(request.sessionId);
        deferredSessionCleanup.add(request.sessionId);
        if (!sessionCleanupScheduled) {
          sessionCleanupScheduled = true;
          queue.afterPending(() => {
            for (const sessionId of deferredSessionCleanup) runtime.cancelSession(sessionId);
            deferredSessionCleanup.clear();
            sessionCleanupScheduled = false;
          });
        }
      }
      postFailure(
        request.requestId,
        'INFERENCE_FAILED',
        'The local transcription worker is at capacity.',
      );
    },
  });
  process.parentPort.on('message', (event) => {
    const raw: unknown = event.data;
    const parsed = WhisperWorkerRequestSchema.safeParse(raw);
    if (parsed.success) {
      queue.enqueue(parsed.data);
    } else {
      postFailure(readRequestId(raw), 'PROTOCOL_ERROR', 'The worker request was invalid.');
    }
  });

  process.parentPort.postMessage(
    WhisperWorkerResponseSchema.parse({
      version: 1,
      requestId: 'worker-ready',
      ok: true,
      result: {
        type: 'ready',
        networkGuarded: true,
        networkProbeCompleted: networkGuard.probeCompleted,
      },
    }),
  );
}

async function handleRequest(
  request: WhisperWorkerRequest,
  runtime: WhisperRuntime,
  models: ReadonlyMap<string, ReturnType<typeof ModelManifestSchema.parse>['models'][number]>,
  pressureTimer: ReturnType<typeof setInterval>,
  networkProbeCompleted: boolean,
): Promise<void> {
  try {
    const response = await execute(request, runtime, models, pressureTimer, networkProbeCompleted);
    process.parentPort.postMessage(WhisperWorkerResponseSchema.parse(response));
    if (request.type === 'shutdown') setImmediate(() => process.exit(0));
  } catch (error: unknown) {
    const mapped = mapWorkerError(error);
    postFailure(request.requestId, mapped.code, mapped.message);
  }
}

async function execute(
  request: WhisperWorkerRequest,
  runtime: WhisperRuntime,
  models: ReadonlyMap<string, ReturnType<typeof ModelManifestSchema.parse>['models'][number]>,
  pressureTimer: ReturnType<typeof setInterval>,
  networkProbeCompleted: boolean,
): Promise<WhisperWorkerResponse> {
  const base = { version: 1 as const, requestId: request.requestId, ok: true as const };
  switch (request.type) {
    case 'transcribe':
      return {
        ...base,
        result: {
          type: 'transcription',
          value: await runtime.transcribe(new Float32Array(request.pcm), request.options),
        },
      };
    case 'session-open':
      runtime.openSession(request.sessionId, request.options);
      return { ...base, result: { type: 'acknowledged', operation: 'session-open' } };
    case 'session-push':
      await runtime.pushSession(request.sessionId, new Float32Array(request.pcm));
      return { ...base, result: { type: 'acknowledged', operation: 'session-push' } };
    case 'session-finish':
      return {
        ...base,
        result: { type: 'transcription', value: await runtime.finishSession(request.sessionId) },
      };
    case 'session-cancel':
      runtime.cancelSession(request.sessionId);
      return { ...base, result: { type: 'acknowledged', operation: 'session-cancel' } };
    case 'unload':
      await runtime.unload(request.modelId);
      return { ...base, result: { type: 'acknowledged', operation: 'unload' } };
    case 'health':
      return {
        ...base,
        result: { type: 'ready', networkGuarded: true, networkProbeCompleted },
      };
    case 'model-check': {
      const model = models.get(request.modelId);
      if (model === undefined) throw new WorkerModelVerificationError('MODEL_CORRUPT');
      await runtime.checkModel(request.modelId);
      return { ...base, result: { type: 'model-ready' } };
    }
    case 'memory-pressure':
      await runtime.memoryPressure();
      return { ...base, result: { type: 'acknowledged', operation: 'memory-pressure' } };
    case 'shutdown':
      clearInterval(pressureTimer);
      await runtime.shutdown();
      return { ...base, result: { type: 'acknowledged', operation: 'shutdown' } };
  }
}

function readManifestModel(
  models: ReadonlyMap<string, ReturnType<typeof ModelManifestSchema.parse>['models'][number]>,
  modelId: WhisperModelId,
  revision: string,
): ReturnType<typeof ModelManifestSchema.parse>['models'][number] {
  const model = models.get(modelId);
  if (model?.revision !== revision) throw new WorkerModelVerificationError('MODEL_CORRUPT');
  return model;
}

function postFailure(requestId: string, code: WhisperWorkerErrorCode, message: string): void {
  process.parentPort.postMessage(
    WhisperWorkerResponseSchema.parse({
      version: 1,
      requestId,
      ok: false,
      error: { code, message },
    }),
  );
}

function mapWorkerError(error: unknown): {
  readonly code: WhisperWorkerErrorCode;
  readonly message: string;
} {
  if (error instanceof WorkerModelVerificationError) {
    return {
      code: error.code,
      message:
        error.code === 'MODEL_MISSING'
          ? 'The selected local model is missing or incomplete.'
          : 'The selected local model failed integrity verification.',
    };
  }
  const message = error instanceof Error ? error.message : 'Whisper inference failed.';
  if (/local_files_only|not found locally|could not locate|no such file/i.test(message)) {
    return { code: 'MODEL_MISSING', message: 'The selected local model is missing or incomplete.' };
  }
  if (/PCM|audio|sample/i.test(message)) {
    return { code: 'INVALID_AUDIO', message: 'The PCM audio was invalid.' };
  }
  return { code: 'INFERENCE_FAILED', message: 'Local transcription could not be completed.' };
}

function readRequestId(raw: unknown): string {
  if (
    typeof raw === 'object' &&
    raw !== null &&
    'requestId' in raw &&
    typeof raw.requestId === 'string'
  ) {
    return raw.requestId.slice(0, 80) || 'invalid-request';
  }
  return 'invalid-request';
}

function readNetworkGuardState(): {
  readonly installed: true;
  readonly probeCompleted: boolean;
} {
  const value: unknown = Reflect.get(globalThis, Symbol.for('talking-quill.whisper-network-guard'));
  if (
    typeof value !== 'object' ||
    value === null ||
    !('installed' in value) ||
    value.installed !== true ||
    !('probeCompleted' in value) ||
    typeof value.probeCompleted !== 'boolean' ||
    (process.argv.includes('--network-guard-probe') && !value.probeCompleted)
  ) {
    throw new Error('Whisper worker network guard is unavailable.');
  }
  return { installed: true, probeCompleted: value.probeCompleted };
}

function readRequiredArgument(prefix: string): string {
  const argument = process.argv.find((value) => value.startsWith(prefix));
  const value = argument?.slice(prefix.length).trim();
  if (value === undefined || value.length === 0) {
    throw new Error(`Missing worker argument ${prefix}`);
  }
  return value;
}

function readOptionalPositiveIntegerArgument(prefix: string): number | undefined {
  const argument = process.argv.find((value) => value.startsWith(prefix));
  if (argument === undefined) return undefined;
  const value = Number(argument.slice(prefix.length));
  if (!Number.isSafeInteger(value) || value < 10 || value > 5 * 60 * 1_000) {
    throw new Error(`Invalid worker argument ${prefix}`);
  }
  return value;
}
