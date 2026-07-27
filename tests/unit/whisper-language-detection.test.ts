import { describe, expect, it, vi } from 'vitest';
import { WHISPER_CHUNK_SECONDS, WHISPER_SAMPLE_RATE } from '../../app/src/shared/constants/whisper';
import { WHISPER_SOURCE_LANGUAGES } from '../../app/src/shared/schemas/whisper-languages';
import { withWhisperLanguageDetection } from '../../app/src/workers/whisper/language-detection';

interface TestLogits {
  readonly data: Float32Array;
  readonly dims: readonly number[];
  tolist(): unknown;
}

type TestProcessor = (ids: unknown, logits: TestLogits) => TestLogits;

function fixture(detected = 'ru') {
  const languageIds = Object.fromEntries(
    WHISPER_SOURCE_LANGUAGES.map(([language], index) => [`<|${language}|>`, 100 + index]),
  );
  const detectedId = languageIds[`<|${detected}|>`];
  if (detectedId === undefined) throw new Error('Unknown fixture language');
  const inputFeatures = {
    data: new Float32Array([1]),
    dims: [1, 1],
    tolist: () => [[1]],
    dispose: vi.fn(),
  };
  const generated = {
    data: new BigInt64Array([50_258n, BigInt(detectedId)]),
    dims: [1, 2],
    tolist: () => [[50_258n, BigInt(detectedId)]],
    dispose: vi.fn(),
  };
  const generate = vi.fn((options: Readonly<Record<string, unknown>>) => {
    const processors = options.logits_processor as {
      processors: TestProcessor[];
    };
    const logits = {
      data: new Float32Array(300).fill(1),
      dims: [1, 300],
      tolist: () => [],
    };
    for (const processor of processors.processors) processor([], logits);
    expect(logits.data[0]).toBe(Number.NEGATIVE_INFINITY);
    expect(logits.data[detectedId]).toBe(1);
    return Promise.resolve(generated);
  });
  const processor = vi.fn((pcm: Float32Array) => {
    void pcm;
    return Promise.resolve({ input_features: inputFeatures });
  });
  const callable = vi.fn(() => Promise.resolve({ text: '' }));
  const raw = Object.assign(callable, {
    processor,
    model: {
      generation_config: {
        is_multilingual: true,
        decoder_start_token_id: 50_258,
        lang_to_id: languageIds,
        task_to_id: { transcribe: 50_359, translate: 50_358 },
      },
      generate,
    },
  });
  const createList = () => ({
    processors: [] as TestProcessor[],
    push(processor_: TestProcessor) {
      this.processors.push(processor_);
    },
  });
  const pipeline = withWhisperLanguageDetection(raw, createList);
  return { pipeline, processor, generate, generated, inputFeatures };
}

describe('Transformers Whisper language detection', () => {
  it('selects only source-language tokens and returns the generated language', async () => {
    const test = fixture('ru');
    await expect(test.pipeline.detectLanguage?.(new Float32Array([0.1]))).resolves.toBe('ru');
    expect(test.generate).toHaveBeenCalledWith(
      expect.objectContaining({
        decoder_input_ids: [[50_258]],
        max_new_tokens: 1,
        do_sample: false,
      }),
    );
    expect(test.generated.dispose).toHaveBeenCalledOnce();
    expect(test.inputFeatures.dispose).toHaveBeenCalledOnce();
  });

  it('uses at most Whisper’s first 30 seconds for detection', async () => {
    const test = fixture('fr');
    const pcm = new Float32Array(WHISPER_SAMPLE_RATE * (WHISPER_CHUNK_SECONDS + 5));
    await test.pipeline.detectLanguage?.(pcm);
    expect(test.processor.mock.calls[0]?.[0]).toHaveLength(
      WHISPER_SAMPLE_RATE * WHISPER_CHUNK_SECONDS,
    );
  });

  it('fails closed instead of allowing the library’s English fallback', async () => {
    const test = fixture('ru');
    test.generated.tolist = () => [[50_258n, 42n]];
    await expect(test.pipeline.detectLanguage?.(new Float32Array([0.1]))).rejects.toThrow(
      'source-language token',
    );
  });
});
