export class CaptureClientError extends Error {
  readonly code:
    | 'permission-denied'
    | 'no-device'
    | 'device-unavailable'
    | 'unsupported-audio-format'
    | 'worklet-unavailable'
    | 'system-audio-unavailable'
    | 'capture-failed'
    | 'capture-unavailable';

  constructor(code: CaptureClientError['code']) {
    super(code);
    this.name = 'CaptureClientError';
    this.code = code;
  }
}
