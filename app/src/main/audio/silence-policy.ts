import { SESSION_CAP_MS, SILENCE_PRESET_MS, SPEECH_ARMING_MS } from '../../shared/constants/audio';
import type { SilencePreset } from '../../shared/schemas/audio';

export type RecordingDurationMode = 'quick' | 'extended';
export type SilenceStopReason = 'trailing-silence' | 'duration-cap';

export interface LevelObservation {
  readonly rms: number;
  readonly durationMs: number;
  readonly elapsedMs: number;
}

export interface SilencePolicyOptions {
  readonly mode: RecordingDurationMode;
  readonly preset: SilencePreset;
}

const INITIAL_NOISE_FLOOR = 0.004;
const MIN_ENTER_THRESHOLD = 0.012;
const MIN_EXIT_THRESHOLD = 0.008;
const ENTER_NOISE_RATIO = 2.8;
const EXIT_NOISE_RATIO = 1.8;
const NOISE_FLOOR_TIME_CONSTANT_MS = 1_500;

export class SilencePolicy {
  readonly #mode: RecordingDurationMode;
  readonly #trailingSilenceMs: number;
  readonly #capMs: number;
  #noiseFloor = INITIAL_NOISE_FLOOR;
  #speechDurationMs = 0;
  #silenceDurationMs = 0;
  #armed = false;
  #decision: SilenceStopReason | null = null;

  constructor(options: SilencePolicyOptions) {
    this.#mode = options.mode;
    this.#trailingSilenceMs = SILENCE_PRESET_MS[options.preset];
    this.#capMs = SESSION_CAP_MS[options.mode];
  }

  get armed(): boolean {
    return this.#armed;
  }

  get noiseFloor(): number {
    return this.#noiseFloor;
  }

  observe(observation: LevelObservation): SilenceStopReason | null {
    if (this.#decision !== null) return this.#decision;
    validateObservation(observation);
    if (observation.elapsedMs >= this.#capMs) return this.#decide('duration-cap');

    const enterThreshold = Math.max(MIN_ENTER_THRESHOLD, this.#noiseFloor * ENTER_NOISE_RATIO);
    const exitThreshold = Math.max(MIN_EXIT_THRESHOLD, this.#noiseFloor * EXIT_NOISE_RATIO);
    const isSpeech = observation.rms >= enterThreshold;
    const isSilence = observation.rms <= exitThreshold;

    if (!this.#armed) {
      if (isSpeech) {
        this.#speechDurationMs += observation.durationMs;
        if (this.#speechDurationMs >= SPEECH_ARMING_MS) this.#armed = true;
      } else {
        this.#speechDurationMs = 0;
        this.#updateNoiseFloor(observation.rms, observation.durationMs, enterThreshold);
      }
      return null;
    }

    if (this.#mode === 'extended') return null;
    if (isSpeech) this.#silenceDurationMs = 0;
    else if (isSilence) this.#silenceDurationMs += observation.durationMs;

    return this.#silenceDurationMs >= this.#trailingSilenceMs
      ? this.#decide('trailing-silence')
      : null;
  }

  tick(elapsedMs: number): SilenceStopReason | null {
    if (this.#decision !== null) return this.#decision;
    if (!Number.isFinite(elapsedMs) || elapsedMs < 0) throw new Error('Invalid elapsed time');
    return elapsedMs >= this.#capMs ? this.#decide('duration-cap') : null;
  }

  reset(): void {
    this.#noiseFloor = INITIAL_NOISE_FLOOR;
    this.#speechDurationMs = 0;
    this.#silenceDurationMs = 0;
    this.#armed = false;
    this.#decision = null;
  }

  #updateNoiseFloor(rms: number, durationMs: number, enterThreshold: number): void {
    const boundedObservation = Math.min(rms, enterThreshold * 0.9);
    const alpha = 1 - Math.exp(-durationMs / NOISE_FLOOR_TIME_CONSTANT_MS);
    this.#noiseFloor = Math.min(
      0.25,
      Math.max(0.000_1, this.#noiseFloor + alpha * (boundedObservation - this.#noiseFloor)),
    );
  }

  #decide(reason: SilenceStopReason): SilenceStopReason {
    this.#decision = reason;
    return reason;
  }
}

function validateObservation(observation: LevelObservation): void {
  if (
    !Number.isFinite(observation.rms) ||
    observation.rms < 0 ||
    observation.rms > 1 ||
    !Number.isFinite(observation.durationMs) ||
    observation.durationMs <= 0 ||
    !Number.isFinite(observation.elapsedMs) ||
    observation.elapsedMs < 0
  ) {
    throw new Error('Invalid level observation');
  }
}
