import { PCM_FRAME_SAMPLES, PCM_SAMPLE_RATE } from '../../shared/constants/audio';

const FILTER_HALF_TAPS = 16;
const FILTER_TAPS = FILTER_HALF_TAPS * 2;
const MAX_PRECOMPUTED_PHASES = 2_048;
const BUFFER_COMPACTION_THRESHOLD = 4_096;
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
  #frameSumSquares = 0;

  constructor(inputSampleRate: number, onFrame: (frame: ProcessedAudioFrame) => void) {
    this.#resampler = new StreamingResampler(inputSampleRate, PCM_SAMPLE_RATE);
    this.#onFrame = onFrame;
  }

  process(channels: readonly Float32Array[]): void {
    const input = channels.length === 1 ? channels[0] : downmixChannels(channels);
    if (input === undefined || input.length === 0) return;
    this.#append(this.#resampler.process(input));
  }

  flush(): void {
    this.#append(this.#resampler.flush());
    this.#emitFrame();
  }

  #append(samples: Float32Array): void {
    for (const sample of samples) {
      const safe = sanitizeSample(sample);
      this.#frame[this.#frameOffset] = safe;
      this.#frameSumSquares += safe * safe;
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
    const rms = Math.min(1, Math.sqrt(this.#frameSumSquares / this.#frameOffset));
    this.#onFrame({ samples, rms });
    this.#frame = new Float32Array(PCM_FRAME_SAMPLES);
    this.#frameOffset = 0;
    this.#frameSumSquares = 0;
  }
}

export class StreamingResampler {
  readonly #inputRate: number;
  readonly #outputRate: number;
  readonly #step: number;
  readonly #cutoff: number;
  readonly #phaseWeights: readonly Float64Array[] | null;
  #buffer: number[];
  #bufferHead = 0;
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
    this.#phaseWeights = createPhaseWeights(inputRate, outputRate, this.#cutoff);
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
    const targetOutputCount = Math.floor(
      (this.#actualInputCount * this.#outputRate) / this.#inputRate,
    );
    const bufferEnd = this.#bufferStart + this.#buffer.length - this.#bufferHead;
    let producible = targetOutputCount - this.#outputCount;
    if (!flushing) {
      producible = 0;
      while (
        this.#outputCount + producible < targetOutputCount &&
        this.#centerForOutput(this.#outputCount + producible) + FILTER_HALF_TAPS < bufferEnd
      ) {
        producible += 1;
      }
    }
    const output = new Float32Array(producible);
    for (let outputOffset = 0; outputOffset < output.length; outputOffset += 1) {
      output[outputOffset] = this.#interpolate(this.#centerForOutput(this.#outputCount));
      this.#outputCount += 1;
    }
    this.#trimBuffer();
    return output;
  }

  #centerForOutput(outputIndex: number): number {
    return this.#phaseWeights === null
      ? Math.floor(outputIndex * this.#step)
      : Math.floor((outputIndex * this.#inputRate) / this.#outputRate);
  }

  #interpolate(center: number): number {
    const phaseWeights = this.#phaseWeights;
    const weights =
      phaseWeights?.[this.#outputCount % phaseWeights.length] ??
      createFilterWeights(this.#outputCount * this.#step - center, this.#cutoff);
    let weighted = 0;
    const firstSourceIndex = center - FILTER_HALF_TAPS + 1;
    for (let tap = 0; tap < FILTER_TAPS; tap += 1) {
      const sourceIndex = firstSourceIndex + tap;
      const bufferIndex = this.#bufferHead + sourceIndex - this.#bufferStart;
      weighted += (this.#buffer[bufferIndex] ?? 0) * (weights[tap] ?? 0);
    }
    return sanitizeSample(weighted);
  }

  #trimBuffer(): void {
    const retainFrom = this.#centerForOutput(this.#outputCount) - FILTER_HALF_TAPS;
    const removeCount = Math.max(0, retainFrom - this.#bufferStart);
    if (removeCount === 0) return;
    const removed = Math.min(removeCount, this.#buffer.length - this.#bufferHead);
    this.#bufferHead += removed;
    this.#bufferStart += removed;
    if (this.#bufferHead >= BUFFER_COMPACTION_THRESHOLD) {
      this.#buffer = this.#buffer.slice(this.#bufferHead);
      this.#bufferHead = 0;
    }
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

function createPhaseWeights(
  inputRate: number,
  outputRate: number,
  cutoff: number,
): readonly Float64Array[] | null {
  if (!Number.isSafeInteger(inputRate) || !Number.isSafeInteger(outputRate)) return null;
  const divisor = greatestCommonDivisor(inputRate, outputRate);
  const phaseCount = outputRate / divisor;
  if (phaseCount > MAX_PRECOMPUTED_PHASES) return null;
  const inputPhaseStep = inputRate / divisor;
  return Array.from({ length: phaseCount }, (_value, phase) =>
    createFilterWeights(((phase * inputPhaseStep) % phaseCount) / phaseCount, cutoff),
  );
}

function createFilterWeights(fractionalPosition: number, cutoff: number): Float64Array {
  const weights = new Float64Array(FILTER_TAPS);
  let weightSum = 0;
  for (let tap = 0; tap < FILTER_TAPS; tap += 1) {
    const relativeSourceIndex = tap - FILTER_HALF_TAPS + 1;
    const distance = fractionalPosition - relativeSourceIndex;
    const normalizedDistance = distance / FILTER_HALF_TAPS;
    const window =
      Math.abs(normalizedDistance) >= 1 ? 0 : 0.5 * (1 + Math.cos(Math.PI * normalizedDistance));
    const sincArgument = cutoff * distance;
    const sinc =
      Math.abs(sincArgument) < 1e-8
        ? 1
        : Math.sin(Math.PI * sincArgument) / (Math.PI * sincArgument);
    const weight = cutoff * sinc * window;
    weights[tap] = weight;
    weightSum += weight;
  }
  if (weightSum !== 0) {
    for (let tap = 0; tap < weights.length; tap += 1) {
      weights[tap] = (weights[tap] ?? 0) / weightSum;
    }
  }
  return weights;
}

function greatestCommonDivisor(first: number, second: number): number {
  let left = first;
  let right = second;
  while (right !== 0) {
    const remainder = left % right;
    left = right;
    right = remainder;
  }
  return left;
}

function sanitizeSample(sample: number): number {
  return Number.isFinite(sample) ? Math.min(1, Math.max(-1, sample)) : 0;
}
