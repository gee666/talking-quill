export type CaptureFailureCode =
  | 'permission-denied'
  | 'no-device'
  | 'device-unavailable'
  | 'unsupported-audio-format'
  | 'worklet-unavailable'
  | 'system-audio-unavailable'
  | 'capture-failed';

export type CaptureStopReason = 'device-lost' | 'system-audio-lost' | 'error';

export class CaptureEngineError extends Error {
  readonly code: CaptureFailureCode;

  constructor(code: CaptureFailureCode) {
    super(code);
    this.name = 'CaptureEngineError';
    this.code = code;
  }
}

export function captureFailureCode(reason: CaptureStopReason | null): CaptureFailureCode {
  if (reason === 'device-lost') return 'device-unavailable';
  if (reason === 'system-audio-lost') return 'system-audio-unavailable';
  return 'worklet-unavailable';
}

export function allowsDefaultFallback(error: unknown): boolean {
  return (
    error instanceof DOMException &&
    (error.name === 'NotFoundError' ||
      error.name === 'OverconstrainedError' ||
      error.name === 'NotReadableError')
  );
}

export function mapCaptureError(error: unknown): CaptureFailureCode {
  if (error instanceof DOMException) {
    if (error.name === 'NotAllowedError' || error.name === 'SecurityError') {
      return 'permission-denied';
    }
    if (error.name === 'NotFoundError') return 'no-device';
    if (error.name === 'NotReadableError' || error.name === 'OverconstrainedError') {
      return 'device-unavailable';
    }
  }
  return 'capture-failed';
}
