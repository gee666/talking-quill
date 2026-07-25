import { z } from 'zod';

export function utf8ByteLength(value: string): number {
  return new TextEncoder().encode(value).byteLength;
}

export function boundedUtf8String(
  maxCharacters: number,
  maxBytes: number,
  byteMessage = `Text must be at most ${String(maxBytes)} UTF-8 bytes`,
) {
  return z
    .string()
    .max(maxCharacters)
    .refine(
      (value) => value.length > maxCharacters || utf8ByteLength(value) <= maxBytes,
      byteMessage,
    );
}
