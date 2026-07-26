export function discardChunkPrefix(
  chunks: readonly Float32Array[],
  sampleCount: number,
): Float32Array[] {
  if (!Number.isSafeInteger(sampleCount) || sampleCount < 0)
    throw new Error('Discarded sample count is invalid');
  const retained: Float32Array[] = [];
  let remaining = sampleCount;
  for (const chunk of chunks) {
    if (remaining >= chunk.length) {
      remaining -= chunk.length;
      continue;
    }
    if (remaining > 0) {
      retained.push(chunk.slice(remaining));
      remaining = 0;
    } else retained.push(chunk);
  }
  if (remaining !== 0) throw new Error('Cannot discard unavailable PCM samples');
  return retained;
}

export function concatChunks(chunks: readonly Float32Array[], total: number): Float32Array {
  return sliceChunks(chunks, 0, total);
}

export function sliceChunks(
  chunks: readonly Float32Array[],
  start: number,
  count: number,
): Float32Array {
  const output = new Float32Array(count);
  let sourceOffset = 0;
  let outputOffset = 0;
  for (const chunk of chunks) {
    const chunkEnd = sourceOffset + chunk.length;
    if (chunkEnd <= start) {
      sourceOffset = chunkEnd;
      continue;
    }
    const from = Math.max(0, start - sourceOffset);
    const available = Math.min(chunk.length - from, count - outputOffset);
    if (available > 0) output.set(chunk.subarray(from, from + available), outputOffset);
    outputOffset += available;
    sourceOffset = chunkEnd;
    if (outputOffset === count) break;
  }
  if (outputOffset !== count) throw new Error('PCM buffer was inconsistent');
  return output;
}
