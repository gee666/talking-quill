import { closeSync, constants, fstatSync, lstatSync, openSync, readFileSync } from 'node:fs';
import { nativeImage } from 'electron';

const MAX_THUMBNAIL_BYTES = 360 * 1_024;
const MAX_THUMBNAIL_DIMENSION = 4_096;
const THUMBNAIL_JPEG_QUALITY = 80;

/** Reads through a verified descriptor and re-encodes untrusted screenshot bytes. */
export function readVerifiedJpegThumbnail(
  path: string,
  afterLstat?: (path: string) => void,
): Buffer | null {
  let descriptor: number | null = null;
  try {
    const before = lstatSync(path);
    if (
      !before.isFile() ||
      before.isSymbolicLink() ||
      before.size < 4 ||
      before.size > MAX_THUMBNAIL_BYTES
    ) {
      return null;
    }
    afterLstat?.(path);
    descriptor = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW);
    const opened = fstatSync(descriptor);
    if (
      !opened.isFile() ||
      opened.size !== before.size ||
      opened.dev !== before.dev ||
      opened.ino !== before.ino
    ) {
      return null;
    }
    const encoded = readFileSync(descriptor);
    if (
      encoded.length < 4 ||
      encoded[0] !== 0xff ||
      encoded[1] !== 0xd8 ||
      encoded[2] !== 0xff ||
      encoded.at(-2) !== 0xff ||
      encoded.at(-1) !== 0xd9
    ) {
      return null;
    }
    const decoded = nativeImage.createFromBuffer(encoded);
    if (decoded.isEmpty()) return null;
    const size = decoded.getSize();
    if (
      size.width < 1 ||
      size.height < 1 ||
      size.width > MAX_THUMBNAIL_DIMENSION ||
      size.height > MAX_THUMBNAIL_DIMENSION
    ) {
      return null;
    }
    const safe = decoded.toJPEG(THUMBNAIL_JPEG_QUALITY);
    return safe.length >= 4 && safe.length <= MAX_THUMBNAIL_BYTES ? safe : null;
  } catch {
    return null;
  } finally {
    if (descriptor !== null) closeSync(descriptor);
  }
}
