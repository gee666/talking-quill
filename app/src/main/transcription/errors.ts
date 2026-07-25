export type ModelManagerErrorCode =
  | 'CANCELLED'
  | 'CORRUPT'
  | 'DISK_SPACE'
  | 'HTTP'
  | 'FILE_LOCKED'
  | 'OFFLINE'
  | 'PROTOCOL'
  | 'TIMEOUT'
  | 'WORKER_VALIDATION'
  | 'BUSY'
  | 'IO';

export class ModelManagerError extends Error {
  readonly code: ModelManagerErrorCode;
  readonly repairable: boolean;

  constructor(code: ModelManagerErrorCode, message: string, repairable = false) {
    super(message);
    this.name = 'ModelManagerError';
    this.code = code;
    this.repairable = repairable;
  }
}

export type WhisperClientErrorCode =
  | 'CANCELLED'
  | 'WORKER_CRASHED'
  | 'PROTOCOL_ERROR'
  | 'INFERENCE_FAILED'
  | 'MODEL_MISSING'
  | 'MODEL_CORRUPT'
  | 'INVALID_AUDIO';

export class WhisperClientError extends Error {
  readonly code: WhisperClientErrorCode;

  constructor(code: WhisperClientErrorCode, message: string) {
    super(message);
    this.name = 'WhisperClientError';
    this.code = code;
  }
}
