import type { WhisperWorkerResult } from '../../shared/schemas/whisper-protocol';
import type { WhisperClientError } from './errors';

export interface WorkerProcess {
  readonly pid: number | undefined;
  postMessage(message: unknown): void;
  kill(): boolean;
  on(event: 'message', listener: (message: unknown) => void): this;
  on(event: 'exit', listener: (code: number) => void): this;
}

export type WhisperWorkerSpawner = (modulePath: string, args: readonly string[]) => WorkerProcess;

export interface PendingRequest {
  readonly generation: number;
  readonly accepts: (result: WhisperWorkerResult) => boolean;
  readonly resolve: (result: WhisperWorkerResult) => void;
  readonly reject: (error: Error) => void;
}

export interface WorkerRequestOptions {
  readonly timeoutMs: number;
  readonly signal?: AbortSignal | undefined;
  readonly allowClosing?: boolean;
  readonly expectedGeneration?: number | undefined;
  readonly accepts?: (result: WhisperWorkerResult) => boolean;
  readonly captureGeneration?: (generation: number) => void;
  readonly captureRequestId?: (requestId: string) => void;
  readonly onDispatched?: (requestId: string) => void;
}

export type TerminationKind = 'cancel' | 'close' | 'health' | 'protocol' | 'unavailable';

export interface TerminationIntent {
  readonly generation: number;
  kind: TerminationKind;
  error: WhisperClientError;
  restart: boolean;
  readonly settled: Promise<void>;
  readonly cancelledRequestIds: ReadonlySet<string> | null;
  terminationConfirmed: boolean;
  forceTimer: ReturnType<typeof setTimeout> | null;
  retryTimer: ReturnType<typeof setTimeout> | null;
  deadlineTimer: ReturnType<typeof setTimeout> | null;
}

export interface WhisperWorkerSupervisorOptions {
  readonly cacheDirectory: string;
  readonly workerPath?: string | undefined;
  readonly spawn?: WhisperWorkerSpawner | undefined;
  readonly forceKill?: ((pid: number) => void) | undefined;
}
