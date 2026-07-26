import { WHISPER_MAX_SAMPLES, WHISPER_PROTOCOL_VERSION } from '../../shared/constants/whisper';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import {
  TranscriptionOptionsSchema,
  type ModelStatus,
  type TranscriptionOptions,
  type TranscriptionResult,
} from '../../shared/schemas/transcription';
import type {
  WhisperAcknowledgedOperation,
  WhisperWorkerResult,
} from '../../shared/schemas/whisper-protocol';
import { WhisperClientError } from './errors';
import {
  inferenceTimeoutMs,
  openWhisperStreamingSession,
  type WhisperStreamingSession,
} from './whisper-streaming-session';
import {
  CONTROL_REQUEST_TIMEOUT_MS,
  WhisperWorkerSupervisor,
  type WhisperWorkerSpawner,
} from './whisper-worker-supervisor';

const MODEL_CHECK_TIMEOUT_MS = 120_000;

export type { WhisperStreamingSession } from './whisper-streaming-session';
export type { WhisperWorkerSpawner } from './whisper-worker-supervisor';

export interface AcquiredModelUse {
  readonly status: ModelStatus;
  release(): void;
}

export type ModelUseAcquirer = (
  modelId: WhisperModelId,
  signal?: AbortSignal,
) => Promise<AcquiredModelUse>;

export class WhisperWorkerClient {
  readonly #acquireModelUse: ModelUseAcquirer;
  readonly #supervisor: WhisperWorkerSupervisor;
  readonly #maxConcurrentPcmSamples: number;
  #reservedPcmSamples = 0;

  constructor(options: {
    readonly cacheDirectory: string;
    readonly acquireModelUse: ModelUseAcquirer;
    readonly workerPath?: string;
    readonly spawn?: WhisperWorkerSpawner;
    readonly forceKill?: (pid: number) => void;
    /** Bounds caller-owned one-shot PCM retained across active and queued requests. */
    readonly maxConcurrentPcmSamples?: number;
  }) {
    this.#acquireModelUse = options.acquireModelUse;
    this.#supervisor = new WhisperWorkerSupervisor({
      cacheDirectory: options.cacheDirectory,
      workerPath: options.workerPath,
      spawn: options.spawn,
      forceKill: options.forceKill,
    });
    this.#maxConcurrentPcmSamples = boundedSampleCount(
      options.maxConcurrentPcmSamples ?? WHISPER_MAX_SAMPLES,
    );
  }

  async transcribe(
    pcm: Float32Array,
    options: TranscriptionOptions,
    signal: AbortSignal,
  ): Promise<TranscriptionResult> {
    const validated = TranscriptionOptionsSchema.parse(options);
    assertPcmLength(pcm, WHISPER_MAX_SAMPLES, 'PCM exceeds maximum transcription duration.');
    if (this.#reservedPcmSamples + pcm.length > this.#maxConcurrentPcmSamples) {
      throw new WhisperClientError('INVALID_AUDIO', 'Too much one-shot PCM is already queued.');
    }
    this.#reservedPcmSamples += pcm.length;
    try {
      const buffer = copyPcm(pcm);
      const expectedGeneration = this.#supervisor.captureActiveGeneration();
      const use = await this.#acquireReadyModel(validated.modelId, signal);
      let requestGeneration = 0;
      try {
        const result = await this.#supervisor.request(
          (requestId) => ({
            version: WHISPER_PROTOCOL_VERSION,
            requestId,
            type: 'transcribe',
            pcm: buffer,
            options: validated,
          }),
          {
            signal,
            timeoutMs: inferenceTimeoutMs(pcm.length),
            expectedGeneration,
            accepts: isTranscriptionResult,
            captureGeneration: (generation) => {
              requestGeneration = generation;
            },
          },
        );
        if (result.type !== 'transcription') {
          throw new WhisperClientError('PROTOCOL_ERROR', 'Worker returned the wrong response.');
        }
        return result.value;
      } finally {
        this.#supervisor.releaseUseWhenSafe(requestGeneration, () => use.release());
      }
    } finally {
      this.#reservedPcmSamples -= pcm.length;
    }
  }

  async startSession(
    options: TranscriptionOptions,
    signal?: AbortSignal,
  ): Promise<WhisperStreamingSession> {
    const validated = TranscriptionOptionsSchema.parse(options);
    const expectedGeneration = this.#supervisor.captureActiveGeneration();
    const use = await this.#acquireReadyModel(validated.modelId, signal);
    return openWhisperStreamingSession({
      supervisor: this.#supervisor,
      transcriptionOptions: validated,
      expectedGeneration,
      use,
      signal,
    });
  }

  async checkWorkerModel(modelId: WhisperModelId, signal?: AbortSignal): Promise<'ready'> {
    const result = await this.#supervisor.request(
      (requestId) => ({
        version: WHISPER_PROTOCOL_VERSION,
        requestId,
        type: 'model-check',
        modelId,
      }),
      {
        signal,
        timeoutMs: MODEL_CHECK_TIMEOUT_MS,
        accepts: isModelReadyResult,
      },
    );
    if (result.type !== 'model-ready') {
      throw new WhisperClientError(
        'PROTOCOL_ERROR',
        'Worker model check returned the wrong response.',
      );
    }
    return 'ready';
  }

  async unload(modelId?: WhisperModelId): Promise<void> {
    await this.#supervisor.waitForTermination();
    if (!this.#supervisor.hasProcess()) return;
    const result = await this.#supervisor.request(
      (requestId) => ({
        version: WHISPER_PROTOCOL_VERSION,
        requestId,
        type: 'unload',
        ...(modelId === undefined ? {} : { modelId }),
      }),
      {
        timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
        accepts: acceptsAcknowledgement('unload'),
      },
    );
    assertAcknowledged(result, 'unload');
  }

  async memoryPressure(): Promise<void> {
    await this.#supervisor.waitForTermination();
    if (!this.#supervisor.hasProcess()) return;
    const result = await this.#supervisor.request(
      (requestId) => ({
        version: WHISPER_PROTOCOL_VERSION,
        requestId,
        type: 'memory-pressure',
      }),
      {
        timeoutMs: CONTROL_REQUEST_TIMEOUT_MS,
        accepts: acceptsAcknowledgement('memory-pressure'),
      },
    );
    assertAcknowledged(result, 'memory-pressure');
  }

  close(): Promise<void> {
    return this.#supervisor.close();
  }

  async #acquireReadyModel(
    modelId: WhisperModelId,
    signal?: AbortSignal,
  ): Promise<AcquiredModelUse> {
    const use = await this.#acquireModelUse(modelId, signal);
    if (use.status.state === 'ready') return use;
    use.release();
    if (use.status.state === 'corrupt') {
      throw new WhisperClientError('MODEL_CORRUPT', 'The selected local model is corrupt.');
    }
    throw new WhisperClientError('MODEL_MISSING', 'The selected local model is not installed.');
  }
}

function acceptsAcknowledgement(
  operation: WhisperAcknowledgedOperation,
): (result: WhisperWorkerResult) => boolean {
  return (result) => result.type === 'acknowledged' && result.operation === operation;
}

function isTranscriptionResult(result: WhisperWorkerResult): boolean {
  return result.type === 'transcription';
}

function isModelReadyResult(result: WhisperWorkerResult): boolean {
  return result.type === 'model-ready';
}

function assertAcknowledged(
  result: WhisperWorkerResult,
  operation: WhisperAcknowledgedOperation,
): void {
  if (result.type !== 'acknowledged' || result.operation !== operation) {
    throw new WhisperClientError(
      'PROTOCOL_ERROR',
      `Whisper worker returned the wrong acknowledgement for ${operation}.`,
    );
  }
}

function boundedSampleCount(value: number): number {
  if (!Number.isInteger(value) || value < 1 || value > WHISPER_MAX_SAMPLES) {
    throw new Error('Invalid concurrent PCM sample bound');
  }
  return value;
}

function assertPcmLength(pcm: Float32Array, maximum: number, message: string): void {
  if (pcm.length === 0 || pcm.length > maximum) {
    throw new WhisperClientError('INVALID_AUDIO', message);
  }
}

function copyPcm(pcm: Float32Array): ArrayBuffer {
  const copy = new Float32Array(pcm.length);
  copy.set(pcm);
  return copy.buffer;
}
