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
  #pending = Buffer.alloc(0);

  push(chunk: Buffer): readonly Buffer[] {
    if (chunk.length === 0) return [];
    this.#pending =
      this.#pending.length === 0 ? Buffer.from(chunk) : Buffer.concat([this.#pending, chunk]);

    const frames: Buffer[] = [];
    let offset = 0;
    while (this.#pending.length - offset >= LENGTH_PREFIX_BYTES) {
      const length = this.#pending.readUInt32BE(offset);
      if (length === 0 || length > HELPER_MAX_FRAME_BYTES) {
        throw new HelperFramingError(`Invalid helper frame length: ${String(length)}`);
      }
      const frameEnd = offset + LENGTH_PREFIX_BYTES + length;
      if (this.#pending.length < frameEnd) break;
      frames.push(Buffer.from(this.#pending.subarray(offset + LENGTH_PREFIX_BYTES, frameEnd)));
      offset = frameEnd;
    }

    this.#pending = offset === 0 ? this.#pending : Buffer.from(this.#pending.subarray(offset));
    if (this.#pending.length > HELPER_MAX_FRAME_BYTES + LENGTH_PREFIX_BYTES) {
      throw new HelperFramingError('Helper frame buffer exceeded its bounded payload size');
    }
    return frames;
  }

  finish(): void {
    if (this.#pending.length !== 0) {
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
