import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  WhisperRuntime,
  selectCentralTranscript,
  type WhisperPipeline,
} from '../../app/src/workers/whisper/runtime';

const options = { modelId: 'Xenova/whisper-small' as const, sampleRate: 16_000 as const };
const revisions = {
  'onnx-community/whisper-large-v3-turbo': 'c'.repeat(40),
  'Xenova/whisper-small': 'a'.repeat(40),
};

afterEach(() => vi.useRealTimers());

describe('Whisper runtime', () => {
  it('loads the pinned offline pipeline during model validation without running inference', async () => {
    const pipeline = vi.fn(() => Promise.resolve({ text: '' })) as unknown as WhisperPipeline;
    const factory = vi.fn(() => Promise.resolve(pipeline));
    const runtime = new WhisperRuntime({ cacheDirectory: 'models', revisions, factory });
    await runtime.checkModel('Xenova/whisper-small');
    expect(factory).toHaveBeenCalledWith('Xenova/whisper-small', 'a'.repeat(40), 'models');
    expect(pipeline).not.toHaveBeenCalled();
    await runtime.shutdown();
  });

  it('verifies once on cold load, re-verifies warm checks, and skips warm inference hashing', async () => {
    const pipeline = vi.fn(() => Promise.resolve({ text: 'ok' })) as unknown as WhisperPipeline;
    const verify = vi.fn(() => Promise.resolve());
    const factory = vi.fn(() => Promise.resolve(pipeline));
    const runtime = new WhisperRuntime({ cacheDirectory: 'models', revisions, factory, verify });

    await runtime.checkModel('Xenova/whisper-small');
    await runtime.checkModel('Xenova/whisper-small');
    await runtime.transcribe(new Float32Array([0.1]), options);
    expect(verify).toHaveBeenCalledTimes(2);
    expect(factory).toHaveBeenCalledOnce();
    await runtime.shutdown();

    const coldVerify = vi.fn(() => Promise.resolve());
    const coldRuntime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(pipeline),
      verify: coldVerify,
    });
    await coldRuntime.transcribe(new Float32Array([0.1]), options);
    expect(coldVerify).toHaveBeenCalledOnce();
    await coldRuntime.shutdown();
  });

  it('keeps one warm pipeline and supplies transformers left/right stride chunking', async () => {
    const calls: Readonly<Record<string, unknown>>[] = [];
    let loads = 0;
    const pipeline = ((_pcm: Float32Array, args: Readonly<Record<string, unknown>>) => {
      calls.push(args);
      return Promise.resolve({ text: ' hello world ' });
    }) as WhisperPipeline;
    const runtime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => {
        loads += 1;
        return Promise.resolve(pipeline);
      },
    });
    const cold = await runtime.transcribe(new Float32Array([0.1]), options);
    const warm = await runtime.transcribe(new Float32Array([0.2]), options);
    expect(loads).toBe(1);
    expect(calls).toEqual([
      { task: 'transcribe', chunk_length_s: 30, stride_length_s: 5 },
      { task: 'transcribe', chunk_length_s: 30, stride_length_s: 5 },
    ]);
    expect(cold.pipeline).toMatchObject({ loadCount: 1, reused: false });
    expect(warm.pipeline).toMatchObject({ loadCount: 1, reused: true });
    await runtime.shutdown();
  });

  it('uses 30 second windows at exact 20 second sample hops and timestamp central regions', async () => {
    const starts: number[] = [];
    const streamingCalls: Readonly<Record<string, unknown>>[] = [];
    const outputs = [
      timestampOutput([
        [' start', 1],
        [' repeat', 6],
        [' repeat', 7],
        [' boundary-one', 24],
      ]),
      timestampOutput([
        [' duplicate-overlap', 4],
        [' middle', 6],
        [' boundary-two', 24],
      ]),
      timestampOutput([
        [' duplicate-overlap', 4],
        [' later', 6],
        [' boundary-three', 24],
      ]),
      timestampOutput([
        [' duplicate-overlap', 4],
        [' near-end', 6],
        [' boundary-four', 24],
      ]),
      timestampOutput([
        [' duplicate-overlap', 4],
        [' finish', 6],
      ]),
    ];
    const pipeline = ((pcm: Float32Array, args: Readonly<Record<string, unknown>>) => {
      streamingCalls.push(args);
      const sourceBlock = Math.round((pcm[0] ?? 0) * 10) - 1;
      starts.push(sourceBlock * 10 * 16_000);
      return Promise.resolve(outputs.shift());
    }) as WhisperPipeline;
    const runtime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(pipeline),
    });
    runtime.openSession('session', options);
    for (let block = 0; block < 9; block += 1) {
      const pcm = new Float32Array(10 * 16_000);
      pcm.fill((block + 1) / 10);
      await runtime.pushSession('session', pcm);
    }
    const result = await runtime.finishSession('session');
    expect(starts).toEqual([0, 20 * 16_000, 40 * 16_000, 60 * 16_000, 80 * 16_000]);
    expect(streamingCalls).toHaveLength(5);
    expect(streamingCalls.every((call) => call.return_timestamps === true)).toBe(true);
    expect(streamingCalls.every((call) => call.return_timestamps !== 'word')).toBe(true);
    expect(result.text).toBe(
      'start repeat repeat boundary-one middle boundary-two later boundary-three near-end boundary-four finish',
    );
    expect(result.text.match(/repeat/g)).toHaveLength(2);
    expect(result.text).not.toContain('duplicate-overlap');
  });

  it('snapshots public pushes and reuses one inference-window scratch buffer', async () => {
    const windows: Float32Array[] = [];
    const starts: number[] = [];
    const pipeline = ((pcm: Float32Array) => {
      windows.push(pcm);
      starts.push(pcm[0] ?? 0);
      return Promise.resolve(timestampOutput([]));
    }) as WhisperPipeline;
    const runtime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(pipeline),
    });
    runtime.openSession('owned', options);
    const first = new Float32Array(10 * 16_000).fill(0.1);
    await runtime.pushSession('owned', first);
    first.fill(0.9);
    for (const value of [0.2, 0.3, 0.4, 0.5]) {
      await runtime.pushSession('owned', new Float32Array(10 * 16_000).fill(value));
    }
    expect(starts[0]).toBeCloseTo(0.1);
    expect(starts[1]).toBeCloseTo(0.3);
    expect(windows).toHaveLength(2);
    expect(windows[0]).toBe(windows[1]);
    runtime.cancelSession('owned');
    await runtime.shutdown();
  });

  it('enforces cumulative duration and bounded push payloads', async () => {
    const pipeline = (() => Promise.resolve(timestampOutput([]))) as WhisperPipeline;
    const runtime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(pipeline),
    });
    runtime.openSession('payload', options);
    await expect(runtime.pushSession('payload', new Float32Array(10 * 16_000 + 1))).rejects.toThrow(
      'PCM',
    );

    runtime.openSession('duration', options);
    const tenSeconds = new Float32Array(10 * 16_000);
    tenSeconds.fill(0.1);
    for (let block = 0; block < 180; block += 1) {
      await runtime.pushSession('duration', tenSeconds);
    }
    await expect(runtime.pushSession('duration', new Float32Array([0.1]))).rejects.toThrow(
      'maximum session duration',
    );
  });

  it('unloads after five idle minutes, retains the timer on model mismatch, and defers pressure', async () => {
    vi.useFakeTimers();
    let resolveInference: ((value: unknown) => void) | null = null;
    const dispose = vi.fn(() => Promise.resolve());
    const pipeline = Object.assign(
      () =>
        new Promise<unknown>((resolve) => {
          resolveInference = resolve;
        }),
      { dispose },
    ) as WhisperPipeline;
    const runtime = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(pipeline),
    });
    const inference = runtime.transcribe(new Float32Array([0.1]), options);
    await vi.waitFor(() => expect(resolveInference).not.toBeNull());
    await runtime.memoryPressure();
    expect(dispose).not.toHaveBeenCalled();
    const completeInference = resolveInference as ((value: unknown) => void) | null;
    if (completeInference === null) throw new Error('Inference resolver was not installed');
    completeInference({ text: 'ok' });
    await inference;
    await vi.waitFor(() => expect(dispose).toHaveBeenCalledTimes(1));

    const immediate = Object.assign(() => Promise.resolve({ text: 'ok' }), {
      dispose,
    }) as WhisperPipeline;
    const second = new WhisperRuntime({
      cacheDirectory: 'models',
      revisions,
      factory: () => Promise.resolve(immediate),
    });
    await second.transcribe(new Float32Array([0.2]), options);
    await second.unload('onnx-community/whisper-large-v3-turbo');
    await vi.advanceTimersByTimeAsync(5 * 60 * 1_000);
    expect(dispose).toHaveBeenCalledTimes(2);
  });

  it('waits for pipeline disposal before loading a replacement', async () => {
    let resolveDispose: (() => void) | null = null;
    const dispose = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolveDispose = resolve;
        }),
    );
    let loads = 0;
    const factory = vi.fn(() => {
      loads += 1;
      const inference = (() => Promise.resolve({ text: 'ok' })) as WhisperPipeline;
      return Promise.resolve(loads === 1 ? Object.assign(inference, { dispose }) : inference);
    });
    const runtime = new WhisperRuntime({ cacheDirectory: 'models', revisions, factory });
    await runtime.transcribe(new Float32Array([0.1]), options);

    const unloading = runtime.unload();
    await vi.waitFor(() => expect(dispose).toHaveBeenCalledOnce());
    const reloading = runtime.transcribe(new Float32Array([0.2]), options);
    await Promise.resolve();
    expect(factory).toHaveBeenCalledOnce();
    const finishDispose = resolveDispose as (() => void) | null;
    if (finishDispose === null) throw new Error('Disposal resolver was not installed');
    finishDispose();
    await unloading;
    await expect(reloading).resolves.toMatchObject({ text: 'ok' });
    expect(factory).toHaveBeenCalledTimes(2);
    await runtime.shutdown();
  });

  it('defensively accepts nullable segment timestamp edges', () => {
    expect(
      selectCentralTranscript(
        {
          chunks: [
            { text: ' left-edge', timestamp: [null, 6] },
            { text: ' final-edge', timestamp: [24, null] },
          ],
        },
        5,
        5,
        30,
      ),
    ).toBe(' left-edge final-edge');
    expect(() =>
      selectCentralTranscript({ chunks: [{ text: ' invalid', timestamp: [null, null] }] }, 0, 0, 1),
    ).toThrow('timestamp');
  });

  it('preserves raw English boundary spacing and CJK joins across windows', async () => {
    await expect(streamedText([' Hello', ' world'])).resolves.toBe('Hello world');
    await expect(streamedText(['你好', '世界'])).resolves.toBe('你好世界');
    await expect(streamedText([' go go', ' go'])).resolves.toBe('go go go');
  });

  it('requires valid segment timestamps and selects end boundaries without lexical deduplication', () => {
    expect(() => selectCentralTranscript({ text: 'no chunks' }, 5, 5, 30)).toThrow(
      'segment timestamps',
    );
    expect(
      selectCentralTranscript(
        timestampOutput([
          [' outside-left', 4.9],
          [' first-repeat', 5],
          [' second-repeat', 24.9],
          [' outside-right', 25],
        ]),
        5,
        5,
        30,
      ),
    ).toBe(' first-repeat second-repeat');
  });
});

async function streamedText(parts: readonly [string, string]): Promise<string> {
  const outputs = parts.map((text) => timestampOutput([[text, 6]]));
  const pipeline = (() => Promise.resolve(outputs.shift())) as WhisperPipeline;
  const runtime = new WhisperRuntime({
    cacheDirectory: 'models',
    revisions,
    factory: () => Promise.resolve(pipeline),
  });
  runtime.openSession('spacing', options);
  for (let index = 0; index < 3; index += 1) {
    await runtime.pushSession('spacing', new Float32Array(10 * 16_000));
  }
  return (await runtime.finishSession('spacing')).text;
}

function timestampOutput(entries: readonly (readonly [string, number])[]) {
  return {
    text: entries.map(([text]) => text).join(''),
    chunks: entries.map(([text, end]) => ({
      text,
      timestamp: [Math.max(0, end - 0.2), end],
    })),
  };
}
