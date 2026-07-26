import { CAPTURE_WORKLET_PROCESSOR_NAME } from '../../shared/constants/audio';
import { StreamingPcmProcessor } from './audio-processing';

class CaptureProcessor extends AudioWorkletProcessor {
  readonly #processor: StreamingPcmProcessor;
  #flushed = false;

  constructor() {
    super();
    this.#processor = new StreamingPcmProcessor(sampleRate, ({ samples, rms }) => {
      this.port.postMessage({ type: 'frame', samples, rms }, [samples.buffer]);
    });
    this.port.onmessage = (event: MessageEvent<unknown>) => {
      if (isFlushMessage(event.data)) {
        if (!this.#flushed) this.#processor.flush();
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
    const channels = inputs[0];
    if (!this.#flushed && channels !== undefined) this.#processor.process(channels);
    return true;
  }
}

function isFlushMessage(value: unknown): value is { readonly type: 'flush' } {
  return typeof value === 'object' && value !== null && Reflect.get(value, 'type') === 'flush';
}

registerProcessor(CAPTURE_WORKLET_PROCESSOR_NAME, CaptureProcessor);
