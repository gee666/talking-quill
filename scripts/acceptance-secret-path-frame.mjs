import { readFileSync } from 'node:fs';

const MAX_FRAME_BYTES = 16 * 1024;

export function readAcceptanceSecretPaths(
  readStdin = () => readFileSync(0),
  invalidMessage = 'Acceptance sealing secret-path frame is invalid',
) {
  const bytes = readStdin();
  if (
    !Buffer.isBuffer(bytes) ||
    bytes.length === 0 ||
    bytes.length > MAX_FRAME_BYTES ||
    bytes.at(-1) !== 0x0a
  ) {
    throw new Error(invalidMessage);
  }
  return JSON.parse(bytes.subarray(0, -1).toString('utf8'));
}
