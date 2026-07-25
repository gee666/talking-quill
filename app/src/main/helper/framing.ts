import { HELPER_MAX_FRAME_BYTES } from '../../shared/helper/protocol';

const LENGTH_PREFIX_BYTES = 4;
const utf8Decoder = new TextDecoder('utf-8', { fatal: true });

export class HelperFramingError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'HelperFramingError';
  }
}

export class HelperFrameDecoder {
  readonly #prefix = Buffer.allocUnsafe(LENGTH_PREFIX_BYTES);
  #prefixBytes = 0;
  #payload: Buffer | null = null;
  #payloadBytes = 0;

  push(chunk: Buffer): readonly Buffer[] {
    const frames: Buffer[] = [];
    let offset = 0;
    while (offset < chunk.length) {
      if (this.#payload === null) {
        const prefixBytes = Math.min(
          LENGTH_PREFIX_BYTES - this.#prefixBytes,
          chunk.length - offset,
        );
        chunk.copy(this.#prefix, this.#prefixBytes, offset, offset + prefixBytes);
        this.#prefixBytes += prefixBytes;
        offset += prefixBytes;
        if (this.#prefixBytes !== LENGTH_PREFIX_BYTES) continue;

        const length = this.#prefix.readUInt32BE(0);
        if (length === 0 || length > HELPER_MAX_FRAME_BYTES) {
          throw new HelperFramingError(`Invalid helper frame length: ${String(length)}`);
        }
        this.#payload = Buffer.allocUnsafe(length);
        this.#payloadBytes = 0;
      }

      const payload = this.#payload;
      const payloadBytes = Math.min(payload.length - this.#payloadBytes, chunk.length - offset);
      chunk.copy(payload, this.#payloadBytes, offset, offset + payloadBytes);
      this.#payloadBytes += payloadBytes;
      offset += payloadBytes;
      if (this.#payloadBytes === payload.length) {
        frames.push(payload);
        this.#prefixBytes = 0;
        this.#payload = null;
        this.#payloadBytes = 0;
      }
    }
    return frames;
  }

  finish(): void {
    if (this.#prefixBytes !== 0 || this.#payload !== null) {
      throw new HelperFramingError('Helper stdout ended with a truncated frame');
    }
  }
}

export function encodeHelperFrame(value: unknown): Buffer {
  const payload = Buffer.from(JSON.stringify(value), 'utf8');
  if (payload.length === 0 || payload.length > HELPER_MAX_FRAME_BYTES) {
    throw new HelperFramingError(`Outbound helper frame is ${String(payload.length)} bytes`);
  }
  const frame = Buffer.allocUnsafe(LENGTH_PREFIX_BYTES + payload.length);
  frame.writeUInt32BE(payload.length, 0);
  payload.copy(frame, LENGTH_PREFIX_BYTES);
  return frame;
}

export function decodeHelperJson(payload: Buffer): unknown {
  try {
    return JSON.parse(utf8Decoder.decode(payload)) as unknown;
  } catch {
    throw new HelperFramingError('Helper emitted invalid UTF-8 JSON');
  }
}
