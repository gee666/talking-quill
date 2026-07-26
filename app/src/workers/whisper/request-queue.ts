import type { WhisperWorkerRequest } from '../../shared/schemas/whisper-protocol';

const CONTROL_REQUEST_TYPES = new Set<WhisperWorkerRequest['type']>([
  'memory-pressure',
  'session-cancel',
  'session-finish',
  'shutdown',
  'unload',
]);

export class WhisperRequestQueue {
  readonly #maximumRequests: number;
  readonly #maximumControlRequests: number;
  readonly #maximumPcmBytes: number;
  readonly #execute: (request: WhisperWorkerRequest) => Promise<void>;
  readonly #rejectOverload: (request: WhisperWorkerRequest) => void;
  #tail = Promise.resolve();
  #pendingRequests = 0;
  #pendingPcmBytes = 0;

  constructor(options: {
    readonly maximumRequests: number;
    readonly maximumControlRequests: number;
    readonly maximumPcmBytes: number;
    readonly execute: (request: WhisperWorkerRequest) => Promise<void>;
    readonly rejectOverload: (request: WhisperWorkerRequest) => void;
  }) {
    this.#maximumRequests = options.maximumRequests;
    this.#maximumControlRequests = options.maximumControlRequests;
    this.#maximumPcmBytes = options.maximumPcmBytes;
    this.#execute = options.execute;
    this.#rejectOverload = options.rejectOverload;
  }

  enqueue(request: WhisperWorkerRequest): boolean {
    const pcmBytes = requestPcmBytes(request);
    const control = CONTROL_REQUEST_TYPES.has(request.type);
    const requestLimit = this.#maximumRequests + (control ? this.#maximumControlRequests : 0);
    if (
      this.#pendingRequests >= requestLimit ||
      this.#pendingPcmBytes + pcmBytes > this.#maximumPcmBytes
    ) {
      this.#rejectOverload(request);
      return false;
    }

    this.#pendingRequests += 1;
    this.#pendingPcmBytes += pcmBytes;
    this.#tail = this.#tail
      .then(() => this.#execute(request))
      .catch(() => undefined)
      .finally(() => {
        this.#pendingRequests -= 1;
        this.#pendingPcmBytes -= pcmBytes;
      });
    return true;
  }

  afterPending(operation: () => void): void {
    this.#tail = this.#tail.then(operation).catch(() => undefined);
  }
}

function requestPcmBytes(request: WhisperWorkerRequest): number {
  return request.type === 'transcribe' || request.type === 'session-push'
    ? request.pcm.byteLength
    : 0;
}
