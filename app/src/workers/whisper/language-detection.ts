import { WHISPER_CHUNK_SECONDS, WHISPER_SAMPLE_RATE } from '../../shared/constants/whisper';
import {
  WHISPER_SOURCE_LANGUAGES,
  WhisperSourceLanguageSchema,
  type WhisperSourceLanguage,
} from '../../shared/schemas/whisper-languages';
import type { WhisperPipeline } from './runtime';

interface TensorLike {
  readonly data: { [index: number]: number; readonly length: number };
  readonly dims: readonly number[];
  tolist(): unknown;
  dispose?(): void;
}

interface WhisperGenerationConfig {
  readonly is_multilingual?: unknown;
  readonly decoder_start_token_id?: unknown;
  readonly lang_to_id?: unknown;
  readonly task_to_id?: unknown;
}

interface TransformersWhisperPipeline extends WhisperPipeline {
  readonly processor: (pcm: Float32Array) => Promise<{ readonly input_features: TensorLike }>;
  readonly model: {
    readonly generation_config: WhisperGenerationConfig;
    generate(options: Readonly<Record<string, unknown>>): Promise<TensorLike>;
  };
}

interface LogitsProcessorListLike {
  push(processor: (inputIds: unknown, logits: TensorLike) => TensorLike): void;
}

export type LogitsProcessorListFactory = () => LogitsProcessorListLike;

/**
 * Adds app-owned language detection to Transformers.js's Whisper pipeline. Transformers.js 3.8.1
 * otherwise defaults an omitted language to English; it does not implement Whisper detection.
 */
export function withWhisperLanguageDetection(
  rawPipeline: unknown,
  createLogitsProcessorList: LogitsProcessorListFactory,
): WhisperPipeline {
  if (typeof rawPipeline !== 'function') throw new Error('Whisper pipeline did not load.');
  const pipeline = rawPipeline as TransformersWhisperPipeline;
  validatePipeline(pipeline);
  pipeline.detectLanguage = (pcm) => detectLanguage(pipeline, pcm, createLogitsProcessorList);
  return pipeline;
}

async function detectLanguage(
  pipeline: TransformersWhisperPipeline,
  pcm: Float32Array,
  createLogitsProcessorList: LogitsProcessorListFactory,
): Promise<WhisperSourceLanguage> {
  if (pcm.length === 0) throw new Error('Cannot detect a language from empty audio.');
  const config = validateGenerationConfig(pipeline.model.generation_config);
  const sample = pcm.subarray(0, WHISPER_CHUNK_SECONDS * WHISPER_SAMPLE_RATE);
  const processed = await pipeline.processor(sample);
  const inputFeatures = processed.input_features;
  let generated: TensorLike | null = null;
  try {
    const processors = createLogitsProcessorList();
    const allowedIds = new Set(config.languageByToken.keys());
    processors.push((_inputIds, logits) => {
      const vocabularySize = logits.dims.at(-1);
      if (vocabularySize === undefined || vocabularySize <= 0) {
        throw new Error('Whisper returned invalid language logits.');
      }
      for (let index = 0; index < logits.data.length; index += 1) {
        if (!allowedIds.has(index % vocabularySize)) {
          logits.data[index] = Number.NEGATIVE_INFINITY;
        }
      }
      return logits;
    });

    generated = await pipeline.model.generate({
      inputs: inputFeatures,
      decoder_input_ids: [[config.decoderStartTokenId]],
      logits_processor: processors,
      max_new_tokens: 1,
      do_sample: false,
    });
    const tokenId = readGeneratedToken(generated.tolist());
    const language = config.languageByToken.get(tokenId);
    if (language === undefined) throw new Error('Whisper did not return a source-language token.');
    return language;
  } finally {
    try {
      generated?.dispose?.();
    } finally {
      inputFeatures.dispose?.();
    }
  }
}

function validatePipeline(pipeline: TransformersWhisperPipeline): void {
  if (
    typeof pipeline.processor !== 'function' ||
    (typeof pipeline.model !== 'object' && typeof pipeline.model !== 'function') ||
    typeof pipeline.model.generate !== 'function'
  ) {
    throw new Error('Whisper pipeline does not expose language detection components.');
  }
  validateGenerationConfig(pipeline.model.generation_config);
}

function validateGenerationConfig(config: WhisperGenerationConfig): {
  readonly decoderStartTokenId: number;
  readonly languageByToken: ReadonlyMap<number, WhisperSourceLanguage>;
} {
  if (config.is_multilingual !== true) throw new Error('Whisper model is not multilingual.');
  const decoderStartTokenId = config.decoder_start_token_id;
  if (!Number.isInteger(decoderStartTokenId) || (decoderStartTokenId as number) < 0) {
    throw new Error('Whisper model has no decoder start token.');
  }
  if (!isRecord(config.task_to_id) || !Number.isInteger(config.task_to_id.transcribe)) {
    throw new Error('Whisper model does not support transcription.');
  }
  if (!isRecord(config.lang_to_id)) throw new Error('Whisper model has no language tokens.');

  const languageByToken = new Map<number, WhisperSourceLanguage>();
  for (const [language] of WHISPER_SOURCE_LANGUAGES) {
    const rawTokenId = config.lang_to_id[`<|${language}|>`];
    if (!Number.isInteger(rawTokenId) || (rawTokenId as number) < 0) {
      throw new Error('Whisper model has an invalid language-token inventory.');
    }
    const tokenId = rawTokenId as number;
    if (languageByToken.has(tokenId)) {
      throw new Error('Whisper model has an invalid language-token inventory.');
    }
    languageByToken.set(tokenId, WhisperSourceLanguageSchema.parse(language));
  }
  return {
    decoderStartTokenId: decoderStartTokenId as number,
    languageByToken,
  };
}

function readGeneratedToken(value: unknown): number {
  if (!Array.isArray(value)) {
    throw new Error('Whisper returned an invalid language detection sequence.');
  }
  const firstSequence: unknown = value[0];
  if (!Array.isArray(firstSequence) || firstSequence.length < 2) {
    throw new Error('Whisper returned an invalid language detection sequence.');
  }
  const token: unknown = firstSequence.at(-1);
  const tokenId = typeof token === 'bigint' ? Number(token) : token;
  if (typeof tokenId !== 'number' || !Number.isSafeInteger(tokenId) || tokenId < 0) {
    throw new Error('Whisper returned an invalid language token.');
  }
  return tokenId;
}

function isRecord(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
