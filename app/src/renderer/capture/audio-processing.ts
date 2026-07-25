import { PCM_FRAME_SAMPLES, PCM_SAMPLE_RATE } from '../../shared/constants/audio';

const FILTER_HALF_TAPS = 16;
const CUTOFF_GUARD = 0.94;

export interface ProcessedAudioFrame {
  readonly samples: Float32Array;
  readonly rms: number;
}

export class StreamingPcmProcessor {
  readonly #resampler: StreamingResampler;
  readonly #onFrame: (frame: ProcessedAudioFrame) => void;
  #frame = new Float32Array(PCM_FRAME_SAMPLES);
  #frameOffset = 0;

  constructor(inputSampleRate: number, onFrame: (frame: ProcessedAudioFrame) => void) {
    this.#resampler = new StreamingResampler(inputSampleRate, PCM_SAMPLE_RATE);
    this.#onFrame = onFrame;
  }

  process(channels: readonly Float32Array[]): void {
    if (channels.length === 0) return;
    this.#append(this.#resampler.process(downmixChannels(channels)));
  }

  flush(): void {
    this.#append(this.#resampler.flush());
    this.#emitFrame();
  }

  #append(samples: Float32Array): void {
    for (const sample of samples) {
      this.#frame[this.#frameOffset] = sample;
      this.#frameOffset += 1;
      if (this.#frameOffset === PCM_FRAME_SAMPLES) this.#emitFrame();
    }
  }

  #emitFrame(): void {
    if (this.#frameOffset === 0) return;
    const samples =
      this.#frameOffset === this.#frame.length
        ? this.#frame
        : this.#frame.slice(0, this.#frameOffset);
    this.#onFrame({ samples, rms: calculateRms(samples) });
    this.#frame = new Float32Array(PCM_FRAME_SAMPLES);
    this.#frameOffset = 0;
  }
}

export class StreamingResampler {
  readonly #inputRate: number;
  readonly #outputRate: number;
  readonly #step: number;
  readonly #cutoff: number;
  #buffer: number[];
  #bufferStart = -FILTER_HALF_TAPS;
  #actualInputCount = 0;
  #outputCount = 0;
  #flushed = false;

  constructor(inputRate: number, outputRate: number) {
    if (
      !Number.isFinite(inputRate) ||
      !Number.isFinite(outputRate) ||
      inputRate < outputRate ||
      outputRate <= 0
    ) {
      throw new Error('The capture sample rate cannot be converted to the target rate');
    }
    this.#inputRate = inputRate;
    this.#outputRate = outputRate;
    this.#step = inputRate / outputRate;
    this.#cutoff = Math.min(1, outputRate / inputRate) * CUTOFF_GUARD;
    this.#buffer = Array.from({ length: FILTER_HALF_TAPS }, () => 0);
  }

  process(input: Float32Array): Float32Array {
    if (this.#flushed) throw new Error('Cannot process audio after flush');
    for (const sample of input) this.#buffer.push(sanitizeSample(sample));
    this.#actualInputCount += input.length;
    return this.#drain(false);
  }

  flush(): Float32Array {
    if (this.#flushed) return new Float32Array();
    this.#flushed = true;
    for (let index = 0; index < FILTER_HALF_TAPS + 1; index += 1) this.#buffer.push(0);
    return this.#drain(true);
  }

  #drain(flushing: boolean): Float32Array {
    const output: number[] = [];
    const targetOutputCount = Math.floor(
      (this.#actualInputCount * this.#outputRate) / this.#inputRate,
    );
    const bufferEnd = this.#bufferStart + this.#buffer.length;
    while (this.#outputCount < targetOutputCount) {
      const position = this.#outputCount * this.#step;
      const center = Math.floor(position);
      const lastRequired = center + FILTER_HALF_TAPS;
      if (!flushing && lastRequired >= bufferEnd) break;
      output.push(this.#interpolate(position));
      this.#outputCount += 1;
    }
    this.#trimBuffer();
    return Float32Array.from(output);
  }

  #interpolate(position: number): number {
    const center = Math.floor(position);
    let weighted = 0;
    let weightSum = 0;
    for (
      let sourceIndex = center - FILTER_HALF_TAPS + 1;
      sourceIndex <= center + FILTER_HALF_TAPS;
      sourceIndex += 1
    ) {
      const distance = position - sourceIndex;
      const normalizedDistance = distance / FILTER_HALF_TAPS;
      const window =
        Math.abs(normalizedDistance) >= 1 ? 0 : 0.5 * (1 + Math.cos(Math.PI * normalizedDistance));
      const sincArgument = this.#cutoff * distance;
      const sinc =
        Math.abs(sincArgument) < 1e-8
          ? 1
          : Math.sin(Math.PI * sincArgument) / (Math.PI * sincArgument);
      const weight = this.#cutoff * sinc * window;
      const bufferIndex = sourceIndex - this.#bufferStart;
      const sample = this.#buffer[bufferIndex] ?? 0;
      weighted += sample * weight;
      weightSum += weight;
    }
    return sanitizeSample(weightSum === 0 ? 0 : weighted / weightSum);
  }

  #trimBuffer(): void {
    const nextPosition = this.#outputCount * this.#step;
    const retainFrom = Math.floor(nextPosition) - FILTER_HALF_TAPS;
    const removeCount = Math.max(0, retainFrom - this.#bufferStart);
    if (removeCount === 0) return;
    this.#buffer.splice(0, Math.min(removeCount, this.#buffer.length));
    this.#bufferStart += removeCount;
  }
}

export function downmixChannels(channels: readonly Float32Array[]): Float32Array {
  const sampleCount = channels[0]?.length ?? 0;
  const mixed = new Float32Array(sampleCount);
  if (channels.length === 0) return mixed;
  for (let index = 0; index < sampleCount; index += 1) {
    let value = 0;
    for (const channel of channels) value += sanitizeSample(channel[index] ?? 0);
    mixed[index] = sanitizeSample(value / channels.length);
  }
  return mixed;
}

export function calculateRms(samples: Float32Array): number {
  if (samples.length === 0) return 0;
  let sumSquares = 0;
  for (const sample of samples) {
    const safe = sanitizeSample(sample);
    sumSquares += safe * safe;
  }
  return Math.min(1, Math.sqrt(sumSquares / samples.length));
}

function sanitizeSample(sample: number): number {
  return Number.isFinite(sample) ? Math.min(1, Math.max(-1, sample)) : 0;
}
