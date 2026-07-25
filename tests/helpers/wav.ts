export interface ParsedWav {
  readonly sampleRate: number;
  readonly channels: number;
  readonly pcm: Float32Array;
}

export function parsePcm16Wav(bytes: Uint8Array): ParsedWav {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (
    bytes.byteLength < 44 ||
    readAscii(bytes, 0, 4) !== 'RIFF' ||
    readAscii(bytes, 8, 4) !== 'WAVE'
  ) {
    throw new Error('Invalid RIFF/WAVE fixture');
  }
  let offset = 12;
  let sampleRate: number | null = null;
  let channels: number | null = null;
  let dataOffset: number | null = null;
  let dataLength: number | null = null;
  while (offset + 8 <= bytes.byteLength) {
    const id = readAscii(bytes, offset, 4);
    const length = view.getUint32(offset + 4, true);
    const body = offset + 8;
    if (body + length > bytes.byteLength) throw new Error('Truncated WAV chunk');
    if (id === 'fmt ') {
      if (
        length < 16 ||
        view.getUint16(body, true) !== 1 ||
        view.getUint16(body + 14, true) !== 16
      ) {
        throw new Error('Fixture must use 16-bit PCM');
      }
      channels = view.getUint16(body + 2, true);
      sampleRate = view.getUint32(body + 4, true);
    } else if (id === 'data') {
      dataOffset = body;
      dataLength = length;
    }
    offset = body + length + (length % 2);
  }
  if (sampleRate === null || channels === null || dataOffset === null || dataLength === null) {
    throw new Error('WAV fixture is missing format or data');
  }
  if (channels !== 1 || dataLength % 2 !== 0) throw new Error('Fixture must be mono PCM16');
  const pcm = new Float32Array(dataLength / 2);
  for (let index = 0; index < pcm.length; index += 1) {
    pcm[index] = view.getInt16(dataOffset + index * 2, true) / 32_768;
  }
  return { sampleRate, channels, pcm };
}

function readAscii(bytes: Uint8Array, offset: number, length: number): string {
  return String.fromCharCode(...bytes.subarray(offset, offset + length));
}
