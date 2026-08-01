import { CAPTURE_WORKLET_PROCESSOR_NAME } from '../../shared/constants/audio';
import { mixAudioSources, StreamingPcmProcessor } from './audio-processing';

class CaptureProcessor extends AudioWorkletProcessor {
  readonly #processor: StreamingPcmProcessor;
  readonly #microphoneProcessor: StreamingPcmProcessor;
  readonly #microphoneRms: number[] = [];
  #flushed = false;

  constructor() {
    super();
    this.#microphoneProcessor = new StreamingPcmProcessor(sampleRate, ({ rms }) => {
      this.#microphoneRms.push(rms);
    });
    this.#processor = new StreamingPcmProcessor(sampleRate, ({ samples, rms }) => {
      this.port.postMessage({ type: 'frame', samples, rms: this.#microphoneRms.shift() ?? rms }, [
        samples.buffer,
      ]);
    });
    this.port.onmessage = (event: MessageEvent<unknown>) => {
      if (isFlushMessage(event.data)) {
        if (!this.#flushed) {
          this.#microphoneProcessor.flush();
          this.#processor.flush();
        }
        this.#flushed = true;
        this.port.postMessage({ type: 'flushed' });
      }
    };
  }

  process(
    inputs: readonly (readonly Float32Array[])[],
    outputs: readonly (readonly Float32Array[])[],
  ): boolean {
    for (const output of outputs) for (const channel of output) channel.fill(0);
    const microphone = mixAudioSources([inputs[0] ?? []]);
    const mixed = mixAudioSources(inputs);
    if (!this.#flushed && microphone.length > 0 && mixed.length > 0) {
      this.#microphoneProcessor.process([microphone]);
      this.#processor.process([mixed]);
    }
    return true;
  }
}

function isFlushMessage(value: unknown): value is { readonly type: 'flush' } {
  return typeof value === 'object' && value !== null && Reflect.get(value, 'type') === 'flush';
}

registerProcessor(CAPTURE_WORKLET_PROCESSOR_NAME, CaptureProcessor);
