import {
  WHISPER_CHUNK_SECONDS,
  WHISPER_HOP_SECONDS,
  WHISPER_IDLE_UNLOAD_MS,
  WHISPER_MAX_PUSH_SAMPLES,
  WHISPER_MAX_SAMPLES,
  WHISPER_SAMPLE_RATE,
  WHISPER_STRIDE_SECONDS,
} from '../../shared/constants/whisper';
import type { WhisperModelId } from '../../shared/schemas/model-manifest';
import {
  TranscriptionResultSchema,
  type PipelineReuseMetadata,
  type TranscriptionOptions,
  type TranscriptionResult,
} from '../../shared/schemas/transcription';

export interface WhisperPipeline {
  (pcm: Float32Array, options: Readonly<Record<string, unknown>>): Promise<unknown>;
  dispose?(): Promise<unknown>;
}

export type WhisperPipelineFactory = (
  modelId: WhisperModelId,
  revision: string,
  cacheDirectory: string,
) => Promise<WhisperPipeline>;

export type WhisperModelVerifier = (
  modelId: WhisperModelId,
  revision: string,
  cacheDirectory: string,
) => Promise<void>;

interface LoadedPipeline {
  readonly modelId: WhisperModelId;
  readonly value: WhisperPipeline;
  readonly loadCount: number;
  readonly loadDurationMs: number;
}

interface PipelineCall<Result> {
  readonly value: Result;
  readonly metadata: PipelineReuseMetadata;
}

interface StreamingState {
  readonly options: TranscriptionOptions;
  readonly audio: PcmQueue;
  readonly textParts: string[];
  readonly startedAt: number;
  totalSamples: number;
  processedWindows: number;
  pipelineMetadata: PipelineReuseMetadata | null;
  windowScratch: Float32Array | null;
}

interface TimestampedChunk {
  readonly text: string;
  readonly timestamp: readonly [number | null, number | null];
}

export class WhisperRuntime {
  readonly #cacheDirectory: string;
  readonly #revisions: Readonly<Record<WhisperModelId, string>>;
  readonly #factory: WhisperPipelineFactory;
  readonly #verify: WhisperModelVerifier;
  readonly #sessions = new Map<string, StreamingState>();
  readonly #idleWaiters = new Set<() => void>();
  readonly #idleUnloadMs: number;
  #pipeline: Promise<LoadedPipeline> | null = null;
  #disposal: Promise<void> | null = null;
  #idleTimer: ReturnType<typeof setTimeout> | null = null;
  #busy = 0;
  #pipelineLoadCount = 0;
  #memoryPressurePending = false;

  constructor(options: {
    readonly cacheDirectory: string;
    readonly revisions: Readonly<Record<WhisperModelId, string>>;
    readonly factory: WhisperPipelineFactory;
    readonly verify?: WhisperModelVerifier;
    readonly idleUnloadMs?: number;
  }) {
    this.#cacheDirectory = options.cacheDirectory;
    this.#revisions = options.revisions;
    this.#factory = options.factory;
    this.#verify = options.verify ?? (() => Promise.resolve());
    this.#idleUnloadMs = options.idleUnloadMs ?? WHISPER_IDLE_UNLOAD_MS;
  }

  async checkModel(modelId: WhisperModelId): Promise<void> {
    await this.#withPipeline(modelId, (_pipeline, reused) =>
      reused
        ? this.#verify(modelId, this.#revisions[modelId], this.#cacheDirectory)
        : Promise.resolve(),
    );
  }

  async transcribe(pcm: Float32Array, options: TranscriptionOptions): Promise<TranscriptionResult> {
    validatePcm(pcm, WHISPER_MAX_SAMPLES);
    const startedAt = performance.now();
    const call = await this.#withPipeline(options.modelId, (pipeline) =>
      pipeline(pcm, transcriptionArguments(options, true)),
    );
    return resultFromOutput(
      call.value,
      options.modelId,
      performance.now() - startedAt,
      call.metadata,
    );
  }

  openSession(sessionId: string, options: TranscriptionOptions): void {
    if (this.#sessions.has(sessionId)) throw new Error('Streaming session already exists.');
    this.#sessions.set(sessionId, {
      options,
      audio: new PcmQueue(),
      textParts: [],
      startedAt: performance.now(),
      totalSamples: 0,
      processedWindows: 0,
      pipelineMetadata: null,
      windowScratch: null,
    });
  }

  async pushSession(sessionId: string, pcm: Float32Array): Promise<void> {
    validatePcm(pcm, WHISPER_MAX_PUSH_SAMPLES);
    const state = this.#getSession(sessionId);
    if (state.totalSamples + pcm.length > WHISPER_MAX_SAMPLES) {
      throw new Error('PCM exceeds maximum session duration.');
    }
    state.totalSamples += pcm.length;
    state.audio.append(pcm);
    const windowSamples = WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SECONDS;
    const hopSamples = WHISPER_SAMPLE_RATE * WHISPER_HOP_SECONDS;
    while (state.audio.length >= windowSamples) {
      const window = state.windowScratch ?? new Float32Array(windowSamples);
      state.windowScratch = window;
      state.audio.copyTo(window);
      const call = await this.#withPipeline(state.options.modelId, (pipeline) =>
        pipeline(window, streamingArguments(state.options)),
      );
      state.pipelineMetadata = mergePipelineMetadata(state.pipelineMetadata, call.metadata);
      state.textParts.push(
        selectCentralTranscript(
          call.value,
          state.processedWindows === 0 ? 0 : WHISPER_STRIDE_SECONDS,
          WHISPER_STRIDE_SECONDS,
          WHISPER_CHUNK_SECONDS,
        ),
      );
      state.processedWindows += 1;
      state.audio.discard(hopSamples);
    }
  }

  async finishSession(sessionId: string): Promise<TranscriptionResult> {
    const state = this.#getSession(sessionId);
    this.#sessions.delete(sessionId);
    if (state.audio.length > 0) {
      const durationSeconds = state.audio.length / WHISPER_SAMPLE_RATE;
      const call = await this.#withPipeline(state.options.modelId, (pipeline) =>
        pipeline(state.audio.takeAll(), streamingArguments(state.options)),
      );
      state.pipelineMetadata = mergePipelineMetadata(state.pipelineMetadata, call.metadata);
      state.textParts.push(
        selectCentralTranscript(
          call.value,
          state.processedWindows === 0 ? 0 : Math.min(WHISPER_STRIDE_SECONDS, durationSeconds),
          0,
          durationSeconds,
        ),
      );
    }
    if (state.pipelineMetadata === null) throw new Error('Streaming session contained no audio.');
    return TranscriptionResultSchema.parse({
      text: joinTranscriptParts(state.textParts),
      modelId: state.options.modelId,
      durationMs: performance.now() - state.startedAt,
      pipeline: state.pipelineMetadata,
    });
  }

  cancelSession(sessionId: string): void {
    this.#sessions.delete(sessionId);
  }

  async unload(modelId?: WhisperModelId): Promise<void> {
    for (;;) {
      if (this.#busy > 0) {
        await new Promise<void>((resolve) => this.#idleWaiters.add(resolve));
        continue;
      }
      const loaded = this.#pipeline;
      if (loaded === null) {
        if (this.#disposal !== null) await this.#disposal;
        return;
      }
      const resolved = await loaded.catch(() => null);
      if (this.#busy > 0 || this.#pipeline !== loaded) continue;
      if (resolved === null) {
        this.#pipeline = null;
        return;
      }
      if (modelId !== undefined && resolved.modelId !== modelId) {
        this.#armIdleUnload();
        return;
      }
      if (this.#idleTimer !== null) clearTimeout(this.#idleTimer);
      this.#idleTimer = null;
      this.#pipeline = null;
      await this.#disposePipeline(resolved.value);
      return;
    }
  }

  async memoryPressure(): Promise<void> {
    if (this.#busy > 0) {
      this.#memoryPressurePending = true;
      return;
    }
    await this.unload();
  }

  async shutdown(): Promise<void> {
    this.#sessions.clear();
    await this.unload();
  }

  async #withPipeline<Result>(
    modelId: WhisperModelId,
    operation: (pipeline: WhisperPipeline, reused: boolean) => Promise<Result>,
  ): Promise<PipelineCall<Result>> {
    this.#busy += 1;
    if (this.#idleTimer !== null) clearTimeout(this.#idleTimer);
    this.#idleTimer = null;
    try {
      const loaded = await this.#load(modelId);
      return {
        value: await operation(loaded.pipeline.value, loaded.reused),
        metadata: {
          loadCount: loaded.pipeline.loadCount,
          reused: loaded.reused,
          loadDurationMs: loaded.pipeline.loadDurationMs,
        },
      };
    } finally {
      this.#busy -= 1;
      if (this.#busy === 0) {
        for (const resolve of this.#idleWaiters) resolve();
        this.#idleWaiters.clear();
        if (this.#memoryPressurePending) {
          this.#memoryPressurePending = false;
          void this.unload().catch(() => undefined);
        } else {
          this.#armIdleUnload();
        }
      }
    }
  }

  async #load(
    modelId: WhisperModelId,
  ): Promise<{ readonly pipeline: LoadedPipeline; readonly reused: boolean }> {
    if (this.#disposal !== null) await this.#disposal;
    const currentPromise = this.#pipeline;
    const current = currentPromise === null ? null : await currentPromise.catch(() => null);
    if (current?.modelId === modelId) return { pipeline: current, reused: true };
    if (current !== null) {
      if (this.#pipeline === currentPromise) this.#pipeline = null;
      await this.#disposePipeline(current.value);
    }
    const revision = this.#revisions[modelId];
    const loadStartedAt = performance.now();
    const loading = this.#verify(modelId, revision, this.#cacheDirectory)
      .then(() => this.#factory(modelId, revision, this.#cacheDirectory))
      .then((value) => {
        this.#pipelineLoadCount += 1;
        return {
          modelId,
          value,
          loadCount: this.#pipelineLoadCount,
          loadDurationMs: performance.now() - loadStartedAt,
        };
      });
    this.#pipeline = loading;
    try {
      return { pipeline: await loading, reused: false };
    } catch (error: unknown) {
      if (this.#pipeline === loading) this.#pipeline = null;
      throw error;
    }
  }

  async #disposePipeline(pipeline: WhisperPipeline): Promise<void> {
    if (this.#disposal !== null) await this.#disposal;
    const disposal = Promise.resolve()
      .then(() => pipeline.dispose?.())
      .then(() => undefined);
    this.#disposal = disposal;
    try {
      await disposal;
    } finally {
      if (this.#disposal === disposal) this.#disposal = null;
    }
  }

  #armIdleUnload(): void {
    if (this.#busy > 0 || this.#pipeline === null) return;
    if (this.#idleTimer !== null) clearTimeout(this.#idleTimer);
    this.#idleTimer = setTimeout(
      () => void this.unload().catch(() => undefined),
      this.#idleUnloadMs,
    );
    this.#idleTimer.unref();
  }

  #getSession(sessionId: string): StreamingState {
    const state = this.#sessions.get(sessionId);
    if (state === undefined) throw new Error('Streaming session does not exist.');
    return state;
  }
}

export function selectCentralTranscript(
  output: unknown,
  leftStrideSeconds: number,
  rightStrideSeconds: number,
  durationSeconds: number,
): string {
  const chunks = readTimestampedChunks(output);
  const upper = Math.max(leftStrideSeconds, durationSeconds - rightStrideSeconds);
  return chunks
    .filter((chunk) => {
      const edge = timestampSelectionPoint(chunk.timestamp);
      return edge >= leftStrideSeconds && edge < upper;
    })
    .map((chunk) => chunk.text)
    .join('');
}

class PcmQueue {
  readonly #chunks: (Float32Array | null)[] = [];
  #headChunk = 0;
  #headOffset = 0;
  #length = 0;

  get length(): number {
    return this.#length;
  }

  append(pcm: Float32Array): void {
    this.#chunks.push(pcm);
    this.#length += pcm.length;
  }

  copyTo(output: Float32Array): void {
    if (output.length > this.#length) throw new Error('PCM queue underflow.');
    let outputOffset = 0;
    let chunkIndex = this.#headChunk;
    let sourceOffset = this.#headOffset;
    while (outputOffset < output.length) {
      const chunk = this.#chunks[chunkIndex];
      if (chunk === undefined || chunk === null) throw new Error('PCM queue is inconsistent.');
      const count = Math.min(chunk.length - sourceOffset, output.length - outputOffset);
      output.set(chunk.subarray(sourceOffset, sourceOffset + count), outputOffset);
      outputOffset += count;
      chunkIndex += 1;
      sourceOffset = 0;
    }
  }

  discard(samples: number): void {
    if (samples > this.#length) throw new Error('PCM queue underflow.');
    let remaining = samples;
    while (remaining > 0) {
      const head = this.#chunks[this.#headChunk];
      if (head === undefined || head === null) throw new Error('PCM queue is inconsistent.');
      const available = head.length - this.#headOffset;
      if (remaining < available) {
        this.#headOffset += remaining;
        remaining = 0;
      } else {
        remaining -= available;
        this.#chunks[this.#headChunk] = null;
        this.#headChunk += 1;
        this.#headOffset = 0;
      }
    }
    this.#length -= samples;
    if (this.#headChunk >= 1_024 && this.#headChunk * 2 >= this.#chunks.length) {
      this.#chunks.splice(0, this.#headChunk);
      this.#headChunk = 0;
    }
  }

  takeAll(): Float32Array {
    const output = new Float32Array(this.#length);
    this.copyTo(output);
    this.#chunks.length = 0;
    this.#headChunk = 0;
    this.#headOffset = 0;
    this.#length = 0;
    return output;
  }
}

function mergePipelineMetadata(
  current: PipelineReuseMetadata | null,
  next: PipelineReuseMetadata,
): PipelineReuseMetadata {
  if (current === null) return next;
  return {
    loadCount: next.loadCount,
    reused: current.reused && next.reused,
    loadDurationMs:
      current.loadCount === next.loadCount
        ? current.loadDurationMs
        : current.loadDurationMs + next.loadDurationMs,
  };
}

function transcriptionArguments(
  options: TranscriptionOptions,
  chunkLongAudio: boolean,
): Readonly<Record<string, unknown>> {
  return {
    task: 'transcribe',
    ...(options.language === undefined ? {} : { language: options.language }),
    ...(chunkLongAudio
      ? { chunk_length_s: WHISPER_CHUNK_SECONDS, stride_length_s: WHISPER_STRIDE_SECONDS }
      : { chunk_length_s: 0 }),
  };
}

function streamingArguments(options: TranscriptionOptions): Readonly<Record<string, unknown>> {
  return {
    ...transcriptionArguments(options, false),
    return_timestamps: true,
  };
}

function validatePcm(pcm: Float32Array, maximumSamples: number): void {
  if (!(pcm instanceof Float32Array) || pcm.length === 0 || pcm.length > maximumSamples) {
    throw new Error('PCM must be non-empty 16 kHz mono Float32 audio.');
  }
  for (const sample of pcm) {
    if (!Number.isFinite(sample) || Math.abs(sample) > 1.25) {
      throw new Error('PCM contains an invalid sample.');
    }
  }
}

function readTimestampedChunks(output: unknown): readonly TimestampedChunk[] {
  if (
    typeof output !== 'object' ||
    output === null ||
    !('chunks' in output) ||
    !Array.isArray(output.chunks)
  ) {
    throw new Error('Whisper streaming output did not contain segment timestamps.');
  }
  const chunks: TimestampedChunk[] = [];
  for (const rawValue of output.chunks) {
    const value: unknown = rawValue;
    if (typeof value !== 'object' || value === null) {
      throw new Error('Whisper returned an invalid timestamp chunk.');
    }
    const record = value as Readonly<Record<string, unknown>>;
    const text = record.text;
    const timestamp = record.timestamp;
    if (
      typeof text !== 'string' ||
      !Array.isArray(timestamp) ||
      timestamp.length !== 2 ||
      !isNullableTimestamp(timestamp[0]) ||
      !isNullableTimestamp(timestamp[1]) ||
      (timestamp[0] === null && timestamp[1] === null) ||
      (timestamp[0] !== null && timestamp[1] !== null && timestamp[1] < timestamp[0])
    ) {
      throw new Error('Whisper returned an invalid timestamp chunk.');
    }
    chunks.push({ text, timestamp: [timestamp[0], timestamp[1]] });
  }
  return chunks;
}

function readText(output: unknown): string {
  if (
    typeof output !== 'object' ||
    output === null ||
    !('text' in output) ||
    typeof output.text !== 'string'
  ) {
    throw new Error('Whisper returned an invalid response.');
  }
  return output.text;
}

function timestampSelectionPoint(timestamp: readonly [number | null, number | null]): number {
  const [start, end] = timestamp;
  if (end !== null) return end;
  if (start !== null) return start;
  throw new Error('Whisper returned an empty timestamp.');
}

function isNullableTimestamp(value: unknown): value is number | null {
  return value === null || (typeof value === 'number' && Number.isFinite(value) && value >= 0);
}

function joinTranscriptParts(parts: readonly string[]): string {
  return parts.join('').trim();
}

function resultFromOutput(
  output: unknown,
  modelId: WhisperModelId,
  durationMs: number,
  pipeline: PipelineReuseMetadata,
): TranscriptionResult {
  return TranscriptionResultSchema.parse({
    text: readText(output).trim(),
    modelId,
    durationMs,
    pipeline,
  });
}
