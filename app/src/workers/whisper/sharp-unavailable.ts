export default function unsupportedImageProcessing(): never {
  throw new Error('Image processing is unavailable in the audio-only Whisper worker.');
}
